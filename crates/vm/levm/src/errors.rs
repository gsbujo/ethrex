use bytes::Bytes;
use ethrex_common::types::Log;
use serde::{Deserialize, Serialize};
use thiserror;

use crate::db::error::DatabaseError;

/// Errors that halt the program
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum VMError {
    #[error("Stack Underflow")]
    StackUnderflow,
    #[error("Stack Overflow")]
    StackOverflow,
    #[error("Invalid Jump")]
    InvalidJump,
    #[error("Opcode Not Allowed In Static Context")]
    OpcodeNotAllowedInStaticContext,
    #[error("Opcode Not Found")]
    OpcodeNotFound,
    #[error("Invalid Bytecode")]
    InvalidBytecode,
    #[error("Invalid Contract Prefix")]
    InvalidContractPrefix,
    #[error("Very Large Number")]
    VeryLargeNumber,
    #[error("Invalid Transaction")]
    InvalidTransaction,
    #[error("Revert Opcode")]
    RevertOpcode,
    #[error("Invalid Opcode")]
    InvalidOpcode,
    #[error("Missing Blob Hashes")]
    MissingBlobHashes,
    #[error("Blob Hash Index Out Of Bounds")]
    BlobHashIndexOutOfBounds,
    #[error("Sender Account Does Not Exist")]
    SenderAccountDoesNotExist,
    #[error("Address Does Not Match An Account")]
    AddressDoesNotMatchAnAccount,
    #[error("Gas price is lower than base fee")]
    GasPriceIsLowerThanBaseFee,
    #[error("Address Already Occupied")]
    AddressAlreadyOccupied,
    #[error("Contract Output Too Big")]
    ContractOutputTooBig,
    #[error("Gas limit price product overflow")]
    GasLimitPriceProductOverflow,
    #[error("Balance Overflow")]
    BalanceOverflow,
    #[error("Balance Underflow")]
    BalanceUnderflow,
    #[error("Gas refunds underflow")]
    GasRefundsUnderflow,
    #[error("Gas refunds overflow")]
    GasRefundsOverflow,
    #[error("Memory size overflows")]
    MemorySizeOverflow,
    #[error("Nonce overflowed")]
    NonceOverflow,
    // OutOfGas
    #[error("Out Of Gas")]
    OutOfGas(#[from] OutOfGasError),
    // Internal
    #[error("Internal error: {0}")]
    Internal(#[from] InternalError),
    #[error("Transaction validation error: {0}")]
    TxValidation(#[from] TxValidationError),
    #[error("Offset out of bounds")]
    OutOfBounds,
    #[error("Precompile execution error: {0}")]
    PrecompileError(#[from] PrecompileError),
    #[error("Database access error: {0}")]
    DatabaseError(#[from] DatabaseError),
}

impl VMError {
    /// These errors are unexpected and indicate critical issues.
    /// They should not cause a transaction to revert silently but instead fail loudly, propagating the error.
    pub fn should_propagate(&self) -> bool {
        matches!(self, VMError::Internal(_)) || matches!(self, VMError::DatabaseError(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum TxValidationError {
    #[error("Sender account should not have bytecode")]
    SenderNotEOA,
    #[error("Insufficient account founds")]
    InsufficientAccountFunds,
    #[error("Nonce is max (overflow)")]
    NonceIsMax,
    #[error("Nonce mismatch: expected {expected}, got {actual}")]
    NonceMismatch { expected: u64, actual: u64 },
    #[error("Initcode size exceeded")]
    InitcodeSizeExceeded,
    #[error("Priority fee is greater than max fee per gas")]
    PriorityGreaterThanMaxFeePerGas,
    #[error("Intrinsic gas too low")]
    IntrinsicGasTooLow,
    #[error("Gas allowance exceeded")]
    GasAllowanceExceeded,
    #[error("Insufficient max fee per gas")]
    InsufficientMaxFeePerGas,
    #[error("Insufficient max fee per blob gas")]
    InsufficientMaxFeePerBlobGas,
    #[error("Type 3 transactions are not supported before the Cancun fork")]
    Type3TxPreFork,
    #[error("Type 3 transaction without blobs")]
    Type3TxZeroBlobs,
    #[error("Invalid blob versioned hash")]
    Type3TxInvalidBlobVersionedHash,
    #[error("Blob count exceeded")]
    Type3TxBlobCountExceeded,
    #[error("Contract creation in blob transaction")]
    Type3TxContractCreation,
    #[error("Type 4 transactions are not supported before the Prague fork")]
    Type4TxPreFork,
    #[error("Empty authorization list in type 4 transaction")]
    Type4TxAuthorizationListIsEmpty,
    #[error("Contract creation in type 4 transaction")]
    Type4TxContractCreation,
    #[error("Gas limit price product overflow")]
    GasLimitPriceProductOverflow,
    #[error("Gas limit is too low")]
    GasLimitTooLow,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
pub enum OutOfGasError {
    #[error("Gas Cost Overflow")]
    GasCostOverflow,
    #[error("Gas Used Overflow")]
    GasUsedOverflow,
    #[error("Creation Cost Is Too High")]
    CreationCostIsTooHigh,
    #[error("Consumed Gas Overflow")]
    ConsumedGasOverflow,
    #[error("Max Gas Limit Exceeded")]
    MaxGasLimitExceeded,
    #[error("Arithmetic operation divided by zero in gas calculation")]
    ArithmeticOperationDividedByZero,
    #[error("Memory Expansion Cost Overflow")]
    MemoryExpansionCostOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum InternalError {
    #[error("Overflowed when incrementing program counter")]
    PCOverflowed,
    #[error("Underflowed when decrementing program counter")]
    PCUnderflowed,
    #[error("Arithmetic operation overflowed")]
    ArithmeticOperationOverflow,
    #[error("Arithmetic operation underflowed")]
    ArithmeticOperationUnderflow,
    #[error("Arithmetic operation divided by zero")]
    ArithmeticOperationDividedByZero,
    #[error("Account should have been cached")]
    AccountShouldHaveBeenCached,
    #[error("Tried to convert one type to another")]
    ConversionError,
    #[error("Division error")]
    DivisionError,
    #[error("Tried to access last call frame but found none")]
    CouldNotAccessLastCallframe, // Last callframe before execution is the same as the first, but after execution the last callframe is actually the initial CF
    #[error("Tried to access blobhash but was out of range")]
    BlobHashOutOfRange,
    #[error("Tried to read from empty code")]
    TriedToIndexEmptyCode,
    #[error("Failed computing CREATE address")]
    CouldNotComputeCreateAddress,
    #[error("Failed computing CREATE2 address")]
    CouldNotComputeCreate2Address,
    #[error("Tried to slice non-existing data")]
    SlicingError,
    #[error("Could not pop callframe")]
    CouldNotPopCallframe,
    #[error("Account not found")]
    AccountNotFound,
    #[error("ExcessBlobGas should not be None")]
    ExcessBlobGasShouldNotBeNone,
    #[error("Error in utils file")]
    UtilsError,
    #[error("PC out of bounds")]
    PCOutOfBounds,
    #[error("Unexpected overflow in gas operation")]
    GasOverflow,
    #[error("Undefined state: {0}")]
    UndefinedState(i32), // This error is temporarily for things that cause an undefined state.
    #[error("Invalid precompile address. Tried to execute a precompile that does not exist.")]
    InvalidPrecompileAddress,
    #[error("Spec Id doesn't match to any fork")]
    InvalidSpecId,
    #[error("Account should had been delegated")]
    AccountNotDelegated,
    #[error("No recipient found for privilege transaction")]
    RecipientNotFoundForPrivilegeTransaction,
    //TODO: Refactor all errors. https://github.com/lambdaclass/ethrex/issues/2886
    #[error("Custom error: {0}")]
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum PrecompileError {
    #[error("Error while parsing the calldata")]
    ParsingInputError,
    #[error("Error while increasing consumed gas")]
    GasConsumedOverflow,
    #[error("There is not enough gas to execute precompiled contract")]
    NotEnoughGas,
    #[error("There was an error evaluating the point")]
    EvaluationError,
    #[error("This is a default error")]
    DefaultError,
    #[error("The G1 point is not in the curve")]
    BLS12381G1PointNotInCurve,
    #[error("The G2 point is not in the curve")]
    BLS12381G2PointNotInCurve,
}

#[derive(Debug, Clone)]
/// Note: "Halt" does not mean "Error during execution" it simply
/// means that the execution stopped. It's not called "Stop" because
/// "Stop" is an Opcode
pub enum OpcodeResult {
    Continue { pc_increment: usize },
    Halt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxResult {
    Success,
    Revert(VMError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub result: TxResult,
    pub gas_used: u64,
    pub gas_refunded: u64,
    pub output: Bytes,
    pub logs: Vec<Log>,
}

impl ExecutionReport {
    pub fn is_success(&self) -> bool {
        matches!(self.result, TxResult::Success)
    }
}
