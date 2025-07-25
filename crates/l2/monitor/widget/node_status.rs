use ethrex_storage::Store;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Row, StatefulWidget, Table, TableState},
};

use crate::{based::sequencer_state::SequencerState, sequencer::errors::MonitorError};

#[derive(Clone)]
pub struct NodeStatusTable {
    pub state: TableState,
    pub items: [(String, String); 5],
    sequencer_state: SequencerState,
}

impl NodeStatusTable {
    pub fn new(sequencer_state: SequencerState) -> Self {
        Self {
            state: TableState::default(),
            items: Default::default(),
            sequencer_state,
        }
    }

    pub async fn on_tick(&mut self, store: &Store) -> Result<(), MonitorError> {
        self.items = Self::refresh_items(&self.sequencer_state, store).await?;
        Ok(())
    }

    async fn refresh_items(
        sequencer_state: &SequencerState,
        store: &Store,
    ) -> Result<[(String, String); 5], MonitorError> {
        let last_update = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let status = sequencer_state.status().await;
        let last_known_batch = "NaN"; // TODO: Implement last known batch retrieval
        let last_known_block = store
            .get_latest_block_number()
            .await
            .map_err(|_| MonitorError::GetLatestBlock)?;
        let follower_nodes = "NaN"; // TODO: Implement follower nodes retrieval

        Ok([
            ("Last Update:".to_string(), last_update),
            ("Status:".to_string(), status.to_string()),
            (
                "Last Known Batch:".to_string(),
                last_known_batch.to_string(),
            ),
            (
                "Last Known Block:".to_string(),
                last_known_block.to_string(),
            ),
            ("Peers:".to_string(), follower_nodes.to_string()),
        ])
    }
}

impl StatefulWidget for &mut NodeStatusTable {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let constraints = vec![Constraint::Percentage(50), Constraint::Percentage(50)];

        let rows = self.items.iter().map(|(key, value)| {
            Row::new(vec![
                Span::styled(key, Style::default()),
                Span::styled(value, Style::default()),
            ])
        });

        let node_status_table = Table::new(rows, constraints).block(
            Block::bordered()
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    "Node Status",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
        );

        node_status_table.render(area, buf, state);
    }
}
