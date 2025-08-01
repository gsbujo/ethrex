#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
use bytes::Bytes;
use ethereum_types::{Address, U256};
use ethrex_common::H160;
use ethrex_common::types::BlockNumber;
use ethrex_l2::sequencer::l1_watcher::PrivilegedTransactionData;
use ethrex_l2_common::calldata::Value;
use ethrex_l2_rpc::{
    clients::{deploy, send_eip1559_transaction},
    signer::{LocalSigner, Signer},
};
use ethrex_l2_sdk::calldata::{self, encode_calldata};
use ethrex_l2_sdk::l1_to_l2_tx_data::L1ToL2TransactionData;
use ethrex_l2_sdk::{
    COMMON_BRIDGE_L2_ADDRESS, bridge_address, claim_erc20withdraw, claim_withdraw,
    compile_contract, deposit_erc20, get_address_alias, get_address_from_secret_key,
    get_erc1967_slot, git_clone, wait_for_transaction_receipt,
};
use ethrex_rpc::{
    clients::eth::{EthClient, eth_sender::Overrides, from_hex_string_to_u256},
    types::{
        block_identifier::{BlockIdentifier, BlockTag},
        receipt::RpcReceipt,
    },
};
use hex::FromHexError;
use keccak_hash::{H256, keccak};
use secp256k1::SecretKey;
use std::{
    fs::{File, read_to_string},
    io::{BufRead, BufReader},
    ops::Mul,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

/// Test the full flow of depositing, depositing with contract call, transferring, and withdrawing funds
/// from L1 to L2 and back.
/// The test can be configured with the following environment variables
///
/// RPC urls:
/// INTEGRATION_TEST_L1_RPC: The url of the l1 rpc server
/// INTEGRATION_TEST_L2_RPC: The url of the l2 rpc server
///
/// Accounts private keys:
/// INTEGRATION_TEST_L1_RICH_WALLET_PRIVATE_KEY: The l1 private key that will make the deposit to the l2 and the transfer to the second l2 account
/// INTEGRATION_TEST_RETURN_TRANSFER_PRIVATE_KEY: The l2 private key that will receive the deposit and the transfer it back to the L1_RICH_WALLET_PRIVATE_KEY
/// ETHREX_DEPLOYER_PRIVATE_KEYS_FILE_PATH: The path to a file with pks that are rich accounts in the l2
///
/// Contract addresses:
/// ETHREX_WATCHER_BRIDGE_ADDRESS: The address of the l1 bridge contract
/// INTEGRATION_TEST_PROPOSER_COINBASE_ADDRESS: The address of the l2 coinbase
///
/// Test parameters:
///
/// INTEGRATION_TEST_DEPOSIT_VALUE: amount in wei to deposit from L1_RICH_WALLET_PRIVATE_KEY to the l2, this amount will be deposited 3 times over the course of the test
/// INTEGRATION_TEST_TRANSFER_VALUE: amount in wei to transfer to INTEGRATION_TEST_RETURN_TRANSFER_PRIVATE_KEY, this amount will be returned to the account
/// INTEGRATION_TEST_WITHDRAW_VALUE: amount in wei to withdraw from the l2 back to the l1 from L1_RICH_WALLET_PRIVATE_KEY this will be done INTEGRATION_TEST_WITHDRAW_COUNT times
/// INTEGRATION_TEST_WITHDRAW_COUNT: amount of withdraw transactions to send
/// INTEGRATION_TEST_SKIP_TEST_TOTAL_ETH: if set the integration test will not check for total eth in the chain, only to be used if we don't know all the accounts that exist in l2
const DEFAULT_L1_RPC: &str = "http://localhost:8545";
const DEFAULT_L2_RPC: &str = "http://localhost:1729";
// 0x941e103320615d394a55708be13e45994c7d93b932b064dbcb2b511fe3254e2e
const DEFAULT_L1_RICH_WALLET_PRIVATE_KEY: H256 = H256([
    0x94, 0x1e, 0x10, 0x33, 0x20, 0x61, 0x5d, 0x39, 0x4a, 0x55, 0x70, 0x8b, 0xe1, 0x3e, 0x45, 0x99,
    0x4c, 0x7d, 0x93, 0xb9, 0x32, 0xb0, 0x64, 0xdb, 0xcb, 0x2b, 0x51, 0x1f, 0xe3, 0x25, 0x4e, 0x2e,
]);
// 0xbcdf20249abf0ed6d944c0288fad489e33f66b3960d9e6229c1cd214ed3bbe31
const DEFAULT_L2_RETURN_TRANSFER_PRIVATE_KEY: H256 = H256([
    0xbc, 0xdf, 0x20, 0x24, 0x9a, 0xbf, 0x0e, 0xd6, 0xd9, 0x44, 0xc0, 0x28, 0x8f, 0xad, 0x48, 0x9e,
    0x33, 0xf6, 0x6b, 0x39, 0x60, 0xd9, 0xe6, 0x22, 0x9c, 0x1c, 0xd2, 0x14, 0xed, 0x3b, 0xbe, 0x31,
]);
// 0x0007a881CD95B1484fca47615B64803dad620C8d
const DEFAULT_PROPOSER_COINBASE_ADDRESS: Address = H160([
    0x00, 0x07, 0xa8, 0x81, 0xcd, 0x95, 0xb1, 0x48, 0x4f, 0xca, 0x47, 0x61, 0x5b, 0x64, 0x80, 0x3d,
    0xad, 0x62, 0x0c, 0x8d,
]);

const L2_GAS_COST_MAX_DELTA: U256 = U256([100_000_000_000_000, 0, 0, 0]);

const DEFAULT_PRIVATE_KEYS_FILE_PATH: &str = "../../fixtures/keys/private_keys_l1.txt";

#[tokio::test]
async fn l2_integration_test() -> Result<(), Box<dyn std::error::Error>> {
    read_env_file_by_config();

    let l1_client = l1_client();
    let l2_client = l2_client();
    let rich_wallet_private_key = l1_rich_wallet_private_key();
    let transfer_return_private_key = l2_return_transfer_private_key();
    let deposit_recipient_address = get_address_from_secret_key(&rich_wallet_private_key)
        .expect("Failed to get address from l1 rich wallet pk");

    test_upgrade(&l1_client, &l2_client).await?;

    test_deposit(
        &l1_client,
        &l2_client,
        &rich_wallet_private_key,
        deposit_recipient_address,
    )
    .await?;

    test_transfer(
        &l2_client,
        &rich_wallet_private_key,
        &transfer_return_private_key,
    )
    .await?;

    test_transfer_with_privileged_tx(
        &l1_client,
        &l2_client,
        &rich_wallet_private_key,
        &transfer_return_private_key,
    )
    .await?;

    test_gas_burning(&l1_client, &rich_wallet_private_key).await?;

    test_privileged_tx_with_contract_call(&l1_client, &l2_client, &rich_wallet_private_key).await?;

    test_privileged_tx_with_contract_call_revert(&l1_client, &l2_client, &rich_wallet_private_key)
        .await?;

    test_privileged_tx_not_enough_balance(
        &l1_client,
        &l2_client,
        &rich_wallet_private_key,
        &transfer_return_private_key,
    )
    .await?;

    test_aliasing(&l1_client, &l2_client, &rich_wallet_private_key).await?;

    test_erc20_roundtrip(&l1_client, &l2_client, &rich_wallet_private_key).await?;

    test_erc20_failed_deposit(&l1_client, &l2_client, &rich_wallet_private_key).await?;

    test_forced_withdrawal(&l1_client, &l2_client, &rich_wallet_private_key).await?;

    let withdrawals_count = std::env::var("INTEGRATION_TEST_WITHDRAW_COUNT")
        .map(|amount| amount.parse().expect("Invalid withdrawal amount value"))
        .unwrap_or(5);

    test_n_withdraws(
        &l1_client,
        &l2_client,
        &rich_wallet_private_key,
        withdrawals_count,
    )
    .await?;

    if std::env::var("INTEGRATION_TEST_SKIP_TEST_TOTAL_ETH").is_err() {
        test_total_eth_l2(&l1_client, &l2_client).await?;
    }

    println!("l2_integration_test is done");
    Ok(())
}

async fn test_upgrade(
    l1_client: &EthClient,
    l2_client: &EthClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let private_key = l1_rich_wallet_private_key();

    let contracts_path = Path::new("contracts");
    get_contract_dependencies(contracts_path);
    let remappings = [(
        "@openzeppelin/contracts",
        contracts_path
            .join("lib/openzeppelin-contracts-upgradeable/lib/openzeppelin-contracts/contracts"),
    )];
    compile_contract(
        contracts_path,
        Path::new("contracts/src/l2/CommonBridgeL2.sol"),
        false,
        Some(&remappings),
    )?;

    let bridge_code = hex::decode(std::fs::read("contracts/solc_out/CommonBridgeL2.bin")?)?;
    let deploy_address = test_deploy(l2_client, &bridge_code, &private_key).await?;

    let impl_slot = get_erc1967_slot("eip1967.proxy.implementation");
    let initial_impl = l2_client
        .get_storage_at(
            COMMON_BRIDGE_L2_ADDRESS,
            impl_slot,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;
    let tx_receipt = test_send(
        l1_client,
        &private_key,
        bridge_address()?,
        "upgradeL2Contract(address,address,uint256,bytes)",
        &[
            Value::Address(COMMON_BRIDGE_L2_ADDRESS),
            Value::Address(deploy_address),
            Value::Uint(U256::from(100_000)),
            Value::Bytes(Bytes::new()),
        ],
    )
    .await;
    let _ = wait_for_l2_deposit_receipt(tx_receipt.block_info.block_number, l1_client, l2_client)
        .await?;
    let final_impl = l2_client
        .get_storage_at(
            COMMON_BRIDGE_L2_ADDRESS,
            impl_slot,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;
    println!("upgraded {initial_impl:#x} -> {final_impl:#x}");
    assert_ne!(initial_impl, final_impl);
    Ok(())
}

/// In this test we deploy a contract on L2 and call it from L1 using the CommonBridge contract.
/// We call the contract by making a deposit from L1 to L2 with the recipient being the rich account.
/// The deposit will trigger the call to the contract.
async fn test_privileged_tx_with_contract_call(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    // pragma solidity ^0.8.27;
    // contract Test {
    //     event NumberSet(uint256 indexed number);
    //     function emitNumber(uint256 _number) public {
    //         emit NumberSet(_number);
    //     }
    // }
    let init_code = hex::decode(
        "6080604052348015600e575f5ffd5b506101008061001c5f395ff3fe6080604052348015600e575f5ffd5b50600436106026575f3560e01c8063f15d140b14602a575b5f5ffd5b60406004803603810190603c919060a4565b6042565b005b807f9ec8254969d1974eac8c74afb0c03595b4ffe0a1d7ad8a7f82ed31b9c854259160405160405180910390a250565b5f5ffd5b5f819050919050565b6086816076565b8114608f575f5ffd5b50565b5f81359050609e81607f565b92915050565b5f6020828403121560b65760b56072565b5b5f60c1848285016092565b9150509291505056fea26469706673582212206f6d360696127c56e2d2a456f3db4a61e30eae0ea9b3af3c900c81ea062e8fe464736f6c634300081c0033",
    )?;

    let deployed_contract_address =
        test_deploy(l2_client, &init_code, rich_wallet_private_key).await?;

    let number_to_emit = U256::from(424242);
    let calldata_to_contract: Bytes =
        calldata::encode_calldata("emitNumber(uint256)", &[Value::Uint(number_to_emit)])?.into();

    // We need to get the block number before the deposit to search for logs later.
    let first_block = l2_client.get_block_number().await?;

    test_call_to_contract_with_deposit(
        l1_client,
        l2_client,
        deployed_contract_address,
        calldata_to_contract,
        rich_wallet_private_key,
    )
    .await?;

    println!("Waiting for event to be emitted");

    let mut block_number = first_block;

    let topic = keccak(b"NumberSet(uint256)");

    while l2_client
        .get_logs(
            first_block,
            block_number,
            deployed_contract_address,
            vec![topic],
        )
        .await
        .is_ok_and(|logs| logs.is_empty())
    {
        println!("Waiting for the event to be built");
        block_number += U256::one();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let logs = l2_client
        .get_logs(
            first_block,
            block_number,
            deployed_contract_address,
            vec![topic],
        )
        .await?;

    let number_emitted = U256::from_big_endian(
        &logs
            .first()
            .unwrap()
            .log
            .topics
            .get(1)
            .unwrap()
            .to_fixed_bytes(),
    );

    assert_eq!(
        number_emitted, number_to_emit,
        "Event emitted with wrong value. Expected 424242, got {number_emitted}"
    );

    Ok(())
}

/// Test the deployment of a contract on L2 and call it from L1 using the CommonBridge contract.
/// The call to the contract should revert but the deposit should be successful.
async fn test_privileged_tx_with_contract_call_revert(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    // pragma solidity ^0.8.27;
    // contract RevertTest {
    //     function revert_call() public {
    //         revert("Reverted");
    //     }
    // }
    let init_code = hex::decode(
        "6080604052348015600e575f5ffd5b506101138061001c5f395ff3fe6080604052348015600e575f5ffd5b50600436106026575f3560e01c806311ebce9114602a575b5f5ffd5b60306032565b005b6040517f08c379a000000000000000000000000000000000000000000000000000000000815260040160629060c1565b60405180910390fd5b5f82825260208201905092915050565b7f52657665727465640000000000000000000000000000000000000000000000005f82015250565b5f60ad600883606b565b915060b682607b565b602082019050919050565b5f6020820190508181035f83015260d68160a3565b905091905056fea2646970667358221220903f571921ce472f979989f9135b8637314b68e080fd70d0da6ede87ad8b5bd564736f6c634300081c0033",
    )?;

    let deployed_contract_address =
        test_deploy(l2_client, &init_code, rich_wallet_private_key).await?;

    let calldata_to_contract: Bytes = calldata::encode_calldata("revert_call()", &[])?.into();

    test_call_to_contract_with_deposit(
        l1_client,
        l2_client,
        deployed_contract_address,
        calldata_to_contract,
        rich_wallet_private_key,
    )
    .await?;

    Ok(())
}

async fn test_erc20_roundtrip(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let token_amount: U256 = U256::from(100);

    let rich_wallet_signer: Signer = LocalSigner::new(*rich_wallet_private_key).into();
    let rich_address = rich_wallet_signer.address();

    let init_code_l1 = hex::decode(std::fs::read(
        "../../fixtures/contracts/ERC20/ERC20.bin/TestToken.bin",
    )?)?;
    let token_l1 = test_deploy_l1(l1_client, &init_code_l1, rich_wallet_private_key).await?;

    let contracts_path = Path::new("contracts");

    get_contract_dependencies(contracts_path);
    let remappings = [(
        "@openzeppelin/contracts",
        contracts_path
            .join("lib/openzeppelin-contracts-upgradeable/lib/openzeppelin-contracts/contracts"),
    )];
    compile_contract(
        contracts_path,
        &contracts_path.join("src/example/L2ERC20.sol"),
        false,
        Some(&remappings),
    )?;
    let init_code_l2_inner = hex::decode(String::from_utf8(std::fs::read(
        "contracts/solc_out/TestTokenL2.bin",
    )?)?)?;
    let init_code_l2 = [
        init_code_l2_inner,
        vec![0u8; 12],
        token_l1.to_fixed_bytes().to_vec(),
    ]
    .concat();
    let token_l2 = test_deploy(l2_client, &init_code_l2, rich_wallet_private_key).await?;

    println!("token l1={token_l1:x}, l2={token_l2:x}");
    test_send(
        l1_client,
        rich_wallet_private_key,
        token_l1,
        "freeMint()",
        &[],
    )
    .await;
    test_send(
        l1_client,
        rich_wallet_private_key,
        token_l1,
        "approve(address,uint256)",
        &[Value::Address(bridge_address()?), Value::Uint(token_amount)],
    )
    .await;
    let initial_balance = test_balance_of(l1_client, token_l1, rich_address).await;
    let deposit_tx = deposit_erc20(
        token_l1,
        token_l2,
        token_amount,
        rich_address,
        &rich_wallet_signer,
        l1_client,
    )
    .await
    .unwrap();
    let res = wait_for_transaction_receipt(deposit_tx, l1_client, 10)
        .await
        .unwrap();
    wait_for_l2_deposit_receipt(res.block_info.block_number, l1_client, l2_client)
        .await
        .unwrap();
    let remaining_l1_balance = test_balance_of(l1_client, token_l1, rich_address).await;
    let l2_balance = test_balance_of(l2_client, token_l2, rich_address).await;
    assert_eq!(initial_balance - remaining_l1_balance, token_amount);
    assert_eq!(l2_balance, token_amount);

    test_send(
        l2_client,
        rich_wallet_private_key,
        token_l2,
        "approve(address,uint256)",
        &[
            Value::Address(COMMON_BRIDGE_L2_ADDRESS),
            Value::Uint(token_amount),
        ],
    )
    .await;
    let res = test_send(
        l2_client,
        rich_wallet_private_key,
        COMMON_BRIDGE_L2_ADDRESS,
        "withdrawERC20(address,address,address,uint256)",
        &[
            Value::Address(token_l1),
            Value::Address(token_l2),
            Value::Address(rich_address),
            Value::Uint(token_amount),
        ],
    )
    .await;
    let proof = l2_client
        .wait_for_message_proof(res.tx_info.transaction_hash, 1000)
        .await;
    let proof = proof.unwrap().into_iter().next().expect("proof not found");

    let on_chain_proposer_address = Address::from_str(
        &std::env::var("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS")
            .expect("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS env var not set"),
    )
    .unwrap();
    while l1_client
        .get_last_verified_batch(on_chain_proposer_address)
        .await
        .unwrap()
        < proof.batch_number
    {
        println!("Withdrawal is not verified on L1 yet");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let withdraw_claim_tx = claim_erc20withdraw(
        token_l1,
        token_l2,
        token_amount,
        &rich_wallet_signer,
        l1_client,
        &proof,
    )
    .await
    .expect("error while claiming");
    wait_for_transaction_receipt(withdraw_claim_tx, l1_client, 5).await?;
    let l1_final_balance = test_balance_of(l1_client, token_l1, rich_address).await;
    let l2_final_balance = test_balance_of(l2_client, token_l2, rich_address).await;
    assert_eq!(initial_balance, l1_final_balance);
    assert!(l2_final_balance.is_zero());
    Ok(())
}

async fn test_aliasing(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let init_code_l1 = hex::decode(std::fs::read("../../fixtures/contracts/caller/Caller.bin")?)?;
    let caller_l1 = test_deploy_l1(l1_client, &init_code_l1, rich_wallet_private_key).await?;
    let send_to_l2_calldata = encode_calldata(
        "sendToL2((address,uint256,uint256,bytes))",
        &[Value::Tuple(vec![
            Value::Address(H160::zero()),
            Value::Uint(U256::from(100_000)),
            Value::Uint(U256::zero()),
            Value::Bytes(Bytes::new()),
        ])],
    )?;
    let receipt_l1 = test_send(
        l1_client,
        rich_wallet_private_key,
        caller_l1,
        "doCall(address,bytes)",
        &[
            Value::Address(bridge_address()?),
            Value::Bytes(send_to_l2_calldata.into()),
        ],
    )
    .await;
    let receipt_l2 =
        wait_for_l2_deposit_receipt(receipt_l1.block_info.block_number, l1_client, l2_client)
            .await
            .unwrap();
    println!(
        "alising {:#x} to {:#x}",
        get_address_alias(caller_l1),
        receipt_l2.tx_info.from
    );
    assert_eq!(receipt_l2.tx_info.from, get_address_alias(caller_l1));
    Ok(())
}

async fn test_erc20_failed_deposit(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let token_amount: U256 = U256::from(100);

    let rich_wallet_signer: Signer = LocalSigner::new(*rich_wallet_private_key).into();
    let rich_address = rich_wallet_signer.address();

    let init_code_l1 = hex::decode(std::fs::read(
        "../../fixtures/contracts/ERC20/ERC20.bin/TestToken.bin",
    )?)?;
    let token_l1 = test_deploy_l1(l1_client, &init_code_l1, rich_wallet_private_key).await?;
    let token_l2 = Address::random(); // will cause deposit to fail

    test_send(
        l1_client,
        rich_wallet_private_key,
        token_l1,
        "freeMint()",
        &[],
    )
    .await;
    test_send(
        l1_client,
        rich_wallet_private_key,
        token_l1,
        "approve(address,uint256)",
        &[Value::Address(bridge_address()?), Value::Uint(token_amount)],
    )
    .await;
    let initial_balance = test_balance_of(l1_client, token_l1, rich_address).await;
    let deposit_tx = deposit_erc20(
        token_l1,
        token_l2,
        token_amount,
        rich_address,
        &rich_wallet_signer,
        l1_client,
    )
    .await
    .unwrap();
    let res = wait_for_transaction_receipt(deposit_tx, l1_client, 10)
        .await
        .unwrap();
    let res = wait_for_l2_deposit_receipt(res.block_info.block_number, l1_client, l2_client)
        .await
        .unwrap();

    let proof = l2_client
        .wait_for_message_proof(res.tx_info.transaction_hash, 1000)
        .await;
    let proof = proof.unwrap().into_iter().next().expect("proof not found");

    let on_chain_proposer_address = Address::from_str(
        &std::env::var("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS")
            .expect("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS env var not set"),
    )
    .unwrap();
    while l1_client
        .get_last_verified_batch(on_chain_proposer_address)
        .await
        .unwrap()
        < proof.batch_number
    {
        println!("Withdrawal is not verified on L1 yet");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let withdraw_claim_tx = claim_erc20withdraw(
        token_l1,
        token_l2,
        token_amount,
        &rich_wallet_signer,
        l1_client,
        &proof,
    )
    .await
    .expect("error while claiming");
    wait_for_transaction_receipt(withdraw_claim_tx, l1_client, 5).await?;
    let l1_final_balance = test_balance_of(l1_client, token_l1, rich_address).await;
    assert_eq!(initial_balance, l1_final_balance);
    Ok(())
}

/// Tests that a withdrawal can be triggered by a privileged transaction
/// This ensures the sequencer can't censor withdrawals without stopping the network
async fn test_forced_withdrawal(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let rich_address = ethrex_l2_sdk::get_address_from_secret_key(rich_wallet_private_key)
        .expect("Failed to get address");
    let l1_initial_balance = l1_client
        .get_balance(rich_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    let l2_initial_balance = l2_client
        .get_balance(rich_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    let transfer_value = U256::from(100);
    let mut l1_gas_costs = 0;

    let calldata = encode_calldata("withdraw(address)", &[Value::Address(rich_address)])?;

    let l1_to_l2_tx_hash = ethrex_l2_sdk::send_l1_to_l2_tx(
        rich_address,
        Some(0),
        None,
        L1ToL2TransactionData::new(
            COMMON_BRIDGE_L2_ADDRESS,
            21000 * 5,
            transfer_value,
            Bytes::from(calldata),
        ),
        &l1_rich_wallet_private_key(),
        bridge_address()?,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 to L2 transaction receipt on L1");

    let l1_to_l2_tx_receipt = wait_for_transaction_receipt(l1_to_l2_tx_hash, l1_client, 5).await?;
    l1_gas_costs +=
        l1_to_l2_tx_receipt.tx_info.gas_used * l1_to_l2_tx_receipt.tx_info.effective_gas_price;
    println!("Waiting for L1 to L2 transaction receipt on L2");

    let res = wait_for_l2_deposit_receipt(
        l1_to_l2_tx_receipt.block_info.block_number,
        l1_client,
        l2_client,
    )
    .await?;

    let l2_final_balance = l2_client
        .get_balance(rich_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let proof = l2_client
        .wait_for_message_proof(res.tx_info.transaction_hash, 1000)
        .await;
    let proof = proof.unwrap().into_iter().next().expect("proof not found");

    let on_chain_proposer_address = Address::from_str(
        &std::env::var("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS")
            .expect("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS env var not set"),
    )
    .unwrap();
    while l1_client
        .get_last_verified_batch(on_chain_proposer_address)
        .await
        .unwrap()
        < proof.batch_number
    {
        println!("Withdrawal is not verified on L1 yet");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let withdraw_claim_tx = claim_withdraw(
        transfer_value,
        rich_address,
        *rich_wallet_private_key,
        l1_client,
        &proof,
    )
    .await
    .expect("error while claiming");
    let res = wait_for_transaction_receipt(withdraw_claim_tx, l1_client, 5).await?;
    l1_gas_costs += res.tx_info.gas_used * res.tx_info.effective_gas_price;

    let l1_final_balance = l1_client
        .get_balance(rich_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    assert_eq!(
        l1_initial_balance + transfer_value - l1_gas_costs,
        l1_final_balance
    );
    assert_eq!(l2_initial_balance - transfer_value, l2_final_balance);
    Ok(())
}

async fn test_balance_of(client: &EthClient, token: Address, user: Address) -> U256 {
    let res = client
        .call(
            token,
            ethrex_l2_sdk::calldata::encode_calldata("balanceOf(address)", &[Value::Address(user)])
                .unwrap()
                .into(),
            Default::default(),
        )
        .await
        .unwrap();
    from_hex_string_to_u256(&res).unwrap()
}

async fn test_send(
    client: &EthClient,
    private_key: &SecretKey,
    to: Address,
    signature: &str,
    data: &[Value],
) -> RpcReceipt {
    let signer: Signer = LocalSigner::new(*private_key).into();
    let tx = client
        .build_eip1559_transaction(
            to,
            signer.address(),
            ethrex_l2_sdk::calldata::encode_calldata(signature, data)
                .unwrap()
                .into(),
            Default::default(),
        )
        .await
        .unwrap();
    let tx_hash = send_eip1559_transaction(client, &tx, &signer)
        .await
        .unwrap();
    ethrex_l2_sdk::wait_for_transaction_receipt(tx_hash, client, 10)
        .await
        .unwrap()
}

async fn test_deposit(
    l1_client: &EthClient,
    l2_client: &EthClient,
    depositor_private_key: &SecretKey,
    deposit_recipient_address: Address,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Fetching initial balances on L1 and L2");

    let depositor = ethrex_l2_sdk::get_address_from_secret_key(depositor_private_key)?;
    let deposit_value = std::env::var("INTEGRATION_TEST_DEPOSIT_VALUE")
        .map(|value| U256::from_dec_str(&value).expect("Invalid deposit value"))
        .unwrap_or(U256::from(1000000000000000000000u128));

    let depositor_l1_initial_balance = l1_client
        .get_balance(depositor, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        depositor_l1_initial_balance >= deposit_value,
        "L1 depositor doesn't have enough balance to deposit"
    );

    let deposit_recipient_l2_initial_balance = l2_client
        .get_balance(
            deposit_recipient_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    let bridge_initial_balance = l1_client
        .get_balance(bridge_address()?, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let fee_vault_balance_before_deposit = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Depositing funds from L1 to L2");

    let deposit_tx_hash = ethrex_l2_sdk::deposit_through_transfer(
        deposit_value,
        deposit_recipient_address,
        depositor_private_key,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 deposit transaction receipt");

    let deposit_tx_receipt =
        ethrex_l2_sdk::wait_for_transaction_receipt(deposit_tx_hash, l1_client, 5).await?;

    let depositor_l1_balance_after_deposit = l1_client
        .get_balance(depositor, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        depositor_l1_balance_after_deposit,
        depositor_l1_initial_balance
            - deposit_value
            - deposit_tx_receipt.tx_info.gas_used * deposit_tx_receipt.tx_info.effective_gas_price,
        "Depositor L1 balance didn't decrease as expected after deposit"
    );

    let bridge_balance_after_deposit = l1_client
        .get_balance(bridge_address()?, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        bridge_balance_after_deposit,
        bridge_initial_balance + deposit_value,
        "Bridge balance didn't increase as expected after deposit"
    );

    println!("Waiting for L2 deposit tx receipt");

    let _ = wait_for_l2_deposit_receipt(
        deposit_tx_receipt.block_info.block_number,
        l1_client,
        l2_client,
    )
    .await?;

    let deposit_recipient_l2_balance_after_deposit = l2_client
        .get_balance(
            deposit_recipient_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    assert_eq!(
        deposit_recipient_l2_balance_after_deposit,
        deposit_recipient_l2_initial_balance + deposit_value,
        "Deposit recipient L2 balance didn't increase as expected after deposit"
    );

    let fee_vault_balance_after_deposit = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        fee_vault_balance_after_deposit, fee_vault_balance_before_deposit,
        "Fee vault balance should not change after deposit"
    );

    Ok(())
}

async fn test_transfer(
    l2_client: &EthClient,
    transferer_private_key: &SecretKey,
    returnerer_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transferring funds on L2");
    let transfer_value = std::env::var("INTEGRATION_TEST_TRANSFER_VALUE")
        .map(|value| U256::from_dec_str(&value).expect("Invalid transfer value"))
        .unwrap_or(U256::from(10000000000u128));
    let transferer_address = get_address_from_secret_key(transferer_private_key)?;
    let returner_address = get_address_from_secret_key(returnerer_private_key)?;

    perform_transfer(
        l2_client,
        transferer_private_key,
        returner_address,
        transfer_value,
    )
    .await?;
    // Only return 99% of the transfer, other amount is for fees
    let return_amount = (transfer_value * 99) / 100;

    perform_transfer(
        l2_client,
        returnerer_private_key,
        transferer_address,
        return_amount,
    )
    .await?;

    Ok(())
}

async fn test_transfer_with_privileged_tx(
    l1_client: &EthClient,
    l2_client: &EthClient,
    transferer_private_key: &SecretKey,
    receiver_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transferring funds on L2 through a deposit");
    let transfer_value = std::env::var("INTEGRATION_TEST_TRANSFER_VALUE")
        .map(|value| U256::from_dec_str(&value).expect("Invalid transfer value"))
        .unwrap_or(U256::from(10000000000u128));
    let transferer_address = get_address_from_secret_key(transferer_private_key)?;
    let receiver_address = get_address_from_secret_key(receiver_private_key)?;

    let receiver_balance_before = l2_client
        .get_balance(receiver_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let l1_to_l2_tx_hash = ethrex_l2_sdk::send_l1_to_l2_tx(
        transferer_address,
        Some(0),
        None,
        L1ToL2TransactionData::new(receiver_address, 21000 * 5, transfer_value, Bytes::new()),
        &l1_rich_wallet_private_key(),
        bridge_address()?,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 to L2 transaction receipt on L1");

    let l1_to_l2_tx_receipt = wait_for_transaction_receipt(l1_to_l2_tx_hash, l1_client, 5).await?;
    println!("Waiting for L1 to L2 transaction receipt on L2");

    let _ = wait_for_l2_deposit_receipt(
        l1_to_l2_tx_receipt.block_info.block_number,
        l1_client,
        l2_client,
    )
    .await?;

    println!("Checking balances after transfer");

    let receiver_balance_after = l2_client
        .get_balance(receiver_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    assert_eq!(
        receiver_balance_after,
        receiver_balance_before + transfer_value
    );
    Ok(())
}

async fn test_gas_burning(
    l1_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transferring funds on L2 through a deposit");
    let rich_address = get_address_from_secret_key(rich_wallet_private_key)?;
    let l2_gas_limit = 2_000_000;
    let l1_extra_gas_limit = 400_000;

    let l1_to_l2_tx_hash = ethrex_l2_sdk::send_l1_to_l2_tx(
        rich_address,
        Some(0),
        Some(l2_gas_limit + l1_extra_gas_limit),
        L1ToL2TransactionData::new(rich_address, l2_gas_limit, U256::zero(), Bytes::new()),
        rich_wallet_private_key,
        bridge_address()?,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 to L2 transaction receipt on L1");

    let l1_to_l2_tx_receipt = wait_for_transaction_receipt(l1_to_l2_tx_hash, l1_client, 5).await?;

    assert!(l1_to_l2_tx_receipt.tx_info.gas_used > l2_gas_limit);
    assert!(l1_to_l2_tx_receipt.tx_info.gas_used < l2_gas_limit + l1_extra_gas_limit);
    Ok(())
}

async fn test_privileged_tx_not_enough_balance(
    l1_client: &EthClient,
    l2_client: &EthClient,
    rich_wallet_private_key: &SecretKey,
    receiver_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Transferring funds on L2 through a deposit");
    let rich_address = get_address_from_secret_key(rich_wallet_private_key)?;
    let receiver_address = get_address_from_secret_key(receiver_private_key)?;

    let balance_sender = l2_client
        .get_balance(rich_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    let balance_before = l2_client
        .get_balance(receiver_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let transfer_value = balance_sender + U256::one();

    let l1_to_l2_tx_hash = ethrex_l2_sdk::send_l1_to_l2_tx(
        rich_address,
        Some(0),
        None,
        L1ToL2TransactionData::new(receiver_address, 21000 * 5, transfer_value, Bytes::new()),
        &l1_rich_wallet_private_key(),
        bridge_address()?,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 to L2 transaction receipt on L1");

    let l1_to_l2_tx_receipt = wait_for_transaction_receipt(l1_to_l2_tx_hash, l1_client, 5).await?;
    println!("Waiting for L1 to L2 transaction receipt on L2");

    let _ = wait_for_l2_deposit_receipt(
        l1_to_l2_tx_receipt.block_info.block_number,
        l1_client,
        l2_client,
    )
    .await?;

    println!("Checking balances after transfer");

    let balance_after = l2_client
        .get_balance(receiver_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;
    assert_eq!(balance_after, balance_before);
    Ok(())
}

async fn perform_transfer(
    l2_client: &EthClient,
    transferer_private_key: &SecretKey,
    transfer_recipient_address: Address,
    transfer_value: U256,
) -> Result<(), Box<dyn std::error::Error>> {
    let transferer_address = ethrex_l2_sdk::get_address_from_secret_key(transferer_private_key)?;

    let transferer_initial_l2_balance = l2_client
        .get_balance(transferer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        transferer_initial_l2_balance >= transfer_value,
        "L2 transferer doesn't have enough balance to transfer"
    );

    let transfer_recipient_initial_balance = l2_client
        .get_balance(
            transfer_recipient_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    let fee_vault_balance_before_transfer = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let transfer_tx = ethrex_l2_sdk::transfer(
        transfer_value,
        transferer_address,
        transfer_recipient_address,
        transferer_private_key,
        l2_client,
    )
    .await?;

    let transfer_tx_receipt =
        ethrex_l2_sdk::wait_for_transaction_receipt(transfer_tx, l2_client, 1000).await?;

    let recoverable_fees_vault_balance = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Recoverable Fees Balance: {recoverable_fees_vault_balance}",);

    println!("Checking balances on L2 after transfer");

    let transferer_l2_balance_after_transfer = l2_client
        .get_balance(transferer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        (transferer_initial_l2_balance - transfer_value)
            .abs_diff(transferer_l2_balance_after_transfer)
            < L2_GAS_COST_MAX_DELTA,
        "L2 transferer balance didn't decrease as expected after transfer. Gas costs were {}/{L2_GAS_COST_MAX_DELTA}",
        (transferer_initial_l2_balance - transfer_value)
            .abs_diff(transferer_l2_balance_after_transfer)
    );

    let transfer_recipient_l2_balance_after_transfer = l2_client
        .get_balance(
            transfer_recipient_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    assert_eq!(
        transfer_recipient_l2_balance_after_transfer,
        transfer_recipient_initial_balance + transfer_value,
        "L2 transfer recipient balance didn't increase as expected after transfer"
    );

    let fee_vault_balance_after_transfer = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let transfer_fees = get_fees_details_l2(transfer_tx_receipt, l2_client).await;

    assert_eq!(
        fee_vault_balance_after_transfer,
        fee_vault_balance_before_transfer + transfer_fees.recoverable_fees,
        "Fee vault balance didn't increase as expected after transfer"
    );

    Ok(())
}

async fn test_n_withdraws(
    l1_client: &EthClient,
    l2_client: &EthClient,
    withdrawer_private_key: &SecretKey,
    n: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Withdraw funds from L2 to L1
    let withdrawer_address = ethrex_l2_sdk::get_address_from_secret_key(withdrawer_private_key)?;
    let withdraw_value = std::env::var("INTEGRATION_TEST_WITHDRAW_VALUE")
        .map(|value| U256::from_dec_str(&value).expect("Invalid withdraw value"))
        .unwrap_or(U256::from(100000000000000000000u128));

    println!("Checking balances on L1 and L2 before withdrawal");

    let withdrawer_l2_balance_before_withdrawal = l2_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        withdrawer_l2_balance_before_withdrawal >= withdraw_value,
        "L2 withdrawer doesn't have enough balance to withdraw"
    );

    let bridge_balance_before_withdrawal = l1_client
        .get_balance(bridge_address()?, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        bridge_balance_before_withdrawal >= withdraw_value,
        "L1 bridge doesn't have enough balance to withdraw"
    );

    let withdrawer_l1_balance_before_withdrawal = l1_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let fee_vault_balance_before_withdrawal = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Withdrawing funds from L2 to L1");

    let mut withdraw_txs = vec![];
    let mut receipts = vec![];

    for x in 1..n + 1 {
        println!("Sending withdraw {x}/{n}");
        let withdraw_tx = ethrex_l2_sdk::withdraw(
            withdraw_value,
            withdrawer_address,
            *withdrawer_private_key,
            l2_client,
        )
        .await?;

        withdraw_txs.push(withdraw_tx);

        let withdraw_tx_receipt =
            ethrex_l2_sdk::wait_for_transaction_receipt(withdraw_tx, l2_client, 1000)
                .await
                .expect("Withdraw tx receipt not found");

        receipts.push(withdraw_tx_receipt);
    }

    println!("Checking balances on L1 and L2 after withdrawal");

    let withdrawer_l2_balance_after_withdrawal = l2_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        (withdrawer_l2_balance_before_withdrawal - withdraw_value * n)
            .abs_diff(withdrawer_l2_balance_after_withdrawal)
            < L2_GAS_COST_MAX_DELTA * n,
        "Withdrawer L2 balance didn't decrease as expected after withdrawal"
    );

    let withdrawer_l1_balance_after_withdrawal = l1_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        withdrawer_l1_balance_after_withdrawal, withdrawer_l1_balance_before_withdrawal,
        "Withdrawer L1 balance should not change after withdrawal"
    );

    let fee_vault_balance_after_withdrawal = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let mut withdraw_fees = U256::zero();
    for receipt in receipts {
        withdraw_fees += get_fees_details_l2(receipt, l2_client)
            .await
            .recoverable_fees;
    }

    assert_eq!(
        fee_vault_balance_after_withdrawal,
        fee_vault_balance_before_withdrawal + withdraw_fees,
        "Fee vault balance didn't increase as expected after withdrawal"
    );

    // We need to wait for all the txs to be included in some batch
    let mut proofs = vec![];
    for (i, tx) in withdraw_txs.clone().into_iter().enumerate() {
        println!("Getting withdrawal proof {}/{n}", i + 1);
        let message_proof = l2_client.wait_for_message_proof(tx, 1000).await?;
        let withdrawal_proof = message_proof
            .into_iter()
            .next()
            .expect("no l1messages in withdrawal");
        proofs.push(withdrawal_proof);
    }

    let on_chain_proposer_address = Address::from_str(
        &std::env::var("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS")
            .expect("ETHREX_COMMITTER_ON_CHAIN_PROPOSER_ADDRESS env var not set"),
    )
    .unwrap();
    for proof in &proofs {
        while l1_client
            .get_last_verified_batch(on_chain_proposer_address)
            .await
            .unwrap()
            < proof.batch_number
        {
            println!("Withdrawal is not verified on L1 yet");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    let mut withdraw_claim_txs_receipts = vec![];

    for (x, proof) in proofs.iter().enumerate() {
        println!("Claiming withdrawal on L1 {x}/{n}");

        let withdraw_claim_tx = ethrex_l2_sdk::claim_withdraw(
            withdraw_value,
            withdrawer_address,
            *withdrawer_private_key,
            l1_client,
            proof,
        )
        .await?;
        let withdraw_claim_tx_receipt =
            wait_for_transaction_receipt(withdraw_claim_tx, l1_client, 5).await?;
        withdraw_claim_txs_receipts.push(withdraw_claim_tx_receipt);
    }

    println!("Checking balances on L1 and L2 after claim");

    let withdrawer_l1_balance_after_claim = l1_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let gas_used_value: u64 = withdraw_claim_txs_receipts
        .iter()
        .map(|x| x.tx_info.gas_used * x.tx_info.effective_gas_price)
        .sum();

    assert_eq!(
        withdrawer_l1_balance_after_claim,
        withdrawer_l1_balance_after_withdrawal + withdraw_value * n - gas_used_value,
        "Withdrawer L1 balance wasn't updated as expected after claim"
    );

    let withdrawer_l2_balance_after_claim = l2_client
        .get_balance(withdrawer_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        withdrawer_l2_balance_after_claim, withdrawer_l2_balance_after_withdrawal,
        "Withdrawer L2 balance should not change after claim"
    );

    let bridge_balance_after_withdrawal = l1_client
        .get_balance(bridge_address()?, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        bridge_balance_after_withdrawal,
        bridge_balance_before_withdrawal - withdraw_value * n,
        "Bridge balance didn't decrease as expected after withdrawal"
    );

    Ok(())
}

async fn test_total_eth_l2(
    l1_client: &EthClient,
    l2_client: &EthClient,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Checking total ETH on L2");

    println!("Fetching rich accounts balance on L2");
    let rich_accounts_balance = get_rich_accounts_balance(l2_client)
        .await
        .expect("Failed to get rich accounts balance");

    let coinbase_balance = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Coinbase balance: {coinbase_balance}");

    let total_eth_on_l2 = rich_accounts_balance + coinbase_balance;

    println!("Total ETH on L2: {rich_accounts_balance} + {coinbase_balance} = {total_eth_on_l2}");

    println!("Checking locked ETH on CommonBridge");

    let bridge_address = bridge_address()?;
    let bridge_locked_eth = l1_client
        .get_balance(bridge_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Bridge locked ETH: {bridge_locked_eth}");

    assert!(
        total_eth_on_l2 <= bridge_locked_eth,
        "Total ETH on L2 ({total_eth_on_l2}) is greater than bridge locked ETH ({bridge_locked_eth})"
    );

    Ok(())
}

async fn test_deploy(
    l2_client: &EthClient,
    init_code: &[u8],
    deployer_private_key: &SecretKey,
) -> Result<Address, Box<dyn std::error::Error>> {
    println!("Deploying contract on L2");

    let deployer: Signer = LocalSigner::new(*deployer_private_key).into();

    let deployer_balance_before_deploy = l2_client
        .get_balance(deployer.address(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let fee_vault_balance_before_deploy = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let (deploy_tx_hash, contract_address) = deploy(
        l2_client,
        &deployer,
        init_code.to_vec().into(),
        Overrides::default(),
    )
    .await?;

    let deploy_tx_receipt =
        ethrex_l2_sdk::wait_for_transaction_receipt(deploy_tx_hash, l2_client, 5).await?;

    let deploy_fees = get_fees_details_l2(deploy_tx_receipt, l2_client).await;

    let deployer_balance_after_deploy = l2_client
        .get_balance(deployer.address(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        deployer_balance_after_deploy,
        deployer_balance_before_deploy - deploy_fees.total_fees,
        "Deployer L2 balance didn't decrease as expected after deploy"
    );

    let fee_vault_balance_after_deploy = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        fee_vault_balance_after_deploy,
        fee_vault_balance_before_deploy + deploy_fees.recoverable_fees,
        "Fee vault balance didn't increase as expected after deploy"
    );

    let deployed_contract_balance = l2_client
        .get_balance(contract_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert!(
        deployed_contract_balance.is_zero(),
        "Deployed contract balance should be zero after deploy"
    );

    Ok(contract_address)
}

async fn test_deploy_l1(
    client: &EthClient,
    init_code: &[u8],
    private_key: &SecretKey,
) -> Result<Address, Box<dyn std::error::Error>> {
    println!("Deploying contract on L1");

    let deployer_signer: Signer = LocalSigner::new(*private_key).into();

    let (deploy_tx_hash, contract_address) = deploy(
        client,
        &deployer_signer,
        init_code.to_vec().into(),
        Overrides::default(),
    )
    .await?;

    ethrex_l2_sdk::wait_for_transaction_receipt(deploy_tx_hash, client, 5).await?;

    Ok(contract_address)
}

async fn test_call_to_contract_with_deposit(
    l1_client: &EthClient,
    l2_client: &EthClient,
    deployed_contract_address: Address,
    calldata_to_contract: Bytes,
    caller_private_key: &SecretKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let caller_address = ethrex_l2_sdk::get_address_from_secret_key(caller_private_key)
        .expect("Failed to get address");

    println!("Checking balances before call");

    let caller_l1_balance_before_call = l1_client
        .get_balance(caller_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    let deployed_contract_balance_before_call = l2_client
        .get_balance(
            deployed_contract_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    let fee_vault_balance_before_call = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    println!("Calling contract on L2 with deposit");

    let l1_to_l2_tx_hash = ethrex_l2_sdk::send_l1_to_l2_tx(
        caller_address,
        Some(0),
        None,
        L1ToL2TransactionData::new(
            deployed_contract_address,
            21000 * 5,
            U256::zero(),
            calldata_to_contract.clone(),
        ),
        &l1_rich_wallet_private_key(),
        bridge_address()?,
        l1_client,
    )
    .await?;

    println!("Waiting for L1 to L2 transaction receipt on L1");

    let l1_to_l2_tx_receipt = wait_for_transaction_receipt(l1_to_l2_tx_hash, l1_client, 5).await?;

    println!("Waiting for L1 to L2 transaction receipt on L2");

    let _ = wait_for_l2_deposit_receipt(
        l1_to_l2_tx_receipt.block_info.block_number,
        l1_client,
        l2_client,
    )
    .await?;

    println!("Checking balances after call");

    let caller_l1_balance_after_call = l1_client
        .get_balance(caller_address, BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        caller_l1_balance_after_call,
        caller_l1_balance_before_call
            - l1_to_l2_tx_receipt.tx_info.gas_used
                * l1_to_l2_tx_receipt.tx_info.effective_gas_price,
        "Caller L1 balance didn't decrease as expected after call"
    );

    let fee_vault_balance_after_call = l2_client
        .get_balance(fees_vault(), BlockIdentifier::Tag(BlockTag::Latest))
        .await?;

    assert_eq!(
        fee_vault_balance_after_call, fee_vault_balance_before_call,
        "Fee vault balance increased unexpectedly after call"
    );

    let deployed_contract_balance_after_call = l2_client
        .get_balance(
            deployed_contract_address,
            BlockIdentifier::Tag(BlockTag::Latest),
        )
        .await?;

    assert_eq!(
        deployed_contract_balance_after_call, deployed_contract_balance_before_call,
        "Deployed contract increased unexpectedly after call"
    );

    Ok(())
}

// FIXME: Remove this before merging
#[allow(dead_code)]
#[derive(Debug)]
struct FeesDetails {
    total_fees: U256,
    recoverable_fees: U256,
    burned_fees: U256,
}

async fn get_fees_details_l2(tx_receipt: RpcReceipt, l2_client: &EthClient) -> FeesDetails {
    let total_fees: U256 =
        (tx_receipt.tx_info.gas_used * tx_receipt.tx_info.effective_gas_price).into();

    let effective_gas_price = tx_receipt.tx_info.effective_gas_price;
    let base_fee_per_gas = l2_client
        .get_block_by_number(BlockIdentifier::Number(tx_receipt.block_info.block_number))
        .await
        .unwrap()
        .header
        .base_fee_per_gas
        .unwrap();

    let max_priority_fee_per_gas_transfer: U256 = (effective_gas_price - base_fee_per_gas).into();

    let recoverable_fees = max_priority_fee_per_gas_transfer.mul(tx_receipt.tx_info.gas_used);

    FeesDetails {
        total_fees,
        recoverable_fees,
        burned_fees: total_fees - recoverable_fees,
    }
}

fn l1_client() -> EthClient {
    EthClient::new(&std::env::var("INTEGRATION_TEST_L1_RPC").unwrap_or(DEFAULT_L1_RPC.to_string()))
        .unwrap()
}

fn l2_client() -> EthClient {
    EthClient::new(&std::env::var("INTEGRATION_TEST_L2_RPC").unwrap_or(DEFAULT_L2_RPC.to_string()))
        .unwrap()
}

fn fees_vault() -> Address {
    std::env::var("INTEGRATION_TEST_PROPOSER_COINBASE_ADDRESS")
        .map(|address| address.parse().expect("Invalid proposer coinbase address"))
        .unwrap_or(DEFAULT_PROPOSER_COINBASE_ADDRESS)
}

fn l1_rich_wallet_private_key() -> SecretKey {
    let l1_rich_wallet_pk = std::env::var("INTEGRATION_TEST_L1_RICH_WALLET_PRIVATE_KEY")
        .map(|pk| pk.parse().expect("Invalid l1 rich wallet pk"))
        .unwrap_or(DEFAULT_L1_RICH_WALLET_PRIVATE_KEY);
    SecretKey::from_slice(l1_rich_wallet_pk.as_bytes()).unwrap()
}

fn l2_return_transfer_private_key() -> SecretKey {
    let l2_return_deposit_private_key =
        std::env::var("INTEGRATION_TEST_RETURN_TRANSFER_PRIVATE_KEY")
            .map(|pk| pk.parse().expect("Invalid l1 rich wallet pk"))
            .unwrap_or(DEFAULT_L2_RETURN_TRANSFER_PRIVATE_KEY);
    SecretKey::from_slice(l2_return_deposit_private_key.as_bytes()).unwrap()
}

async fn wait_for_l2_deposit_receipt(
    l1_receipt_block_number: BlockNumber,
    l1_client: &EthClient,
    l2_client: &EthClient,
) -> Result<RpcReceipt, Box<dyn std::error::Error>> {
    let topic = keccak(b"PrivilegedTxSent(address,address,address,uint256,uint256,uint256,bytes)");
    let logs = l1_client
        .get_logs(
            U256::from(l1_receipt_block_number),
            U256::from(l1_receipt_block_number),
            bridge_address()?,
            vec![topic],
        )
        .await?;
    let data = PrivilegedTransactionData::from_log(logs.first().unwrap().log.clone())?;

    let l2_deposit_tx_hash = data
        .into_tx(
            l1_client,
            l2_client.get_chain_id().await?.try_into().unwrap(),
            0,
        )
        .await
        .unwrap()
        .get_privileged_hash()
        .unwrap();

    println!("Waiting for deposit transaction receipt on L2");

    Ok(ethrex_l2_sdk::wait_for_transaction_receipt(l2_deposit_tx_hash, l2_client, 1000).await?)
}

pub fn read_env_file_by_config() {
    let env_file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".env");
    let reader = BufReader::new(File::open(env_file_path).expect("Failed to open .env file"));

    for line in reader.lines() {
        let line = line.expect("Failed to read line");
        if line.starts_with("#") {
            // Skip comments
            continue;
        };
        match line.split_once('=') {
            Some((key, value)) => {
                if std::env::vars().any(|(k, _)| k == key) {
                    continue;
                }
                unsafe { std::env::set_var(key, value) }
            }
            None => continue,
        };
    }
}

async fn get_rich_accounts_balance(
    l2_client: &EthClient,
) -> Result<U256, Box<dyn std::error::Error>> {
    let mut total_balance = U256::zero();
    let private_keys_file_path = private_keys_file_path();

    let pks = read_to_string(private_keys_file_path)?;
    let private_keys: Vec<String> = pks
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect();

    for pk in private_keys.iter() {
        let secret_key = parse_private_key(pk)?;
        let address = get_address_from_secret_key(&secret_key)?;
        let get_balance = l2_client
            .get_balance(address, BlockIdentifier::Tag(BlockTag::Latest))
            .await?;
        total_balance += get_balance;
    }
    Ok(total_balance)
}

fn private_keys_file_path() -> PathBuf {
    match std::env::var("ETHREX_DEPLOYER_PRIVATE_KEYS_FILE_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            println!(
                "ETHREX_DEPLOYER_PRIVATE_KEYS_FILE_PATH not set, using default: {DEFAULT_PRIVATE_KEYS_FILE_PATH}",
            );
            PathBuf::from(DEFAULT_PRIVATE_KEYS_FILE_PATH)
        }
    }
}

pub fn parse_private_key(s: &str) -> Result<SecretKey, Box<dyn std::error::Error>> {
    Ok(SecretKey::from_slice(&parse_hex(s)?)?)
}

pub fn parse_hex(s: &str) -> Result<Bytes, FromHexError> {
    match s.strip_prefix("0x") {
        Some(s) => hex::decode(s).map(Into::into),
        None => hex::decode(s).map(Into::into),
    }
}

fn get_contract_dependencies(contracts_path: &Path) {
    std::fs::create_dir_all(contracts_path.join("lib")).expect("Failed to create contracts/lib");
    git_clone(
        "https://github.com/OpenZeppelin/openzeppelin-contracts-upgradeable.git",
        contracts_path
            .join("lib/openzeppelin-contracts-upgradeable")
            .to_str()
            .expect("Failed to convert path to str"),
        None,
        true,
    )
    .unwrap();
}
