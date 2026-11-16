use clap::Parser;
use mux::pane::PaneId;
use wezterm_client::client::Client;

/// The exit status of a `pane-end` whose multiplexer keeps no end of the
/// pane: it runs and is not closing, or it ended before the ends kept.
pub const NO_END: i32 = 3;

#[derive(Debug, Parser, Clone)]
pub struct PaneEnd {
    /// The pane whose end to print. Unlike other commands, it has no
    /// default: the pane is usually gone.
    #[arg(long)]
    pane_id: PaneId,
}

impl PaneEnd {
    pub async fn run(&self, client: Client) -> anyhow::Result<()> {
        let pane_id = self.pane_id;
        let answer = client.get_pane_end(codec::GetPaneEnd { pane_id }).await?;
        let Some(end) = answer.end else {
            eprintln!(
                "the multiplexer keeps no end of pane {pane_id}: it runs and is not closing, or \
                 it ended before the last {} pane ends the multiplexer keeps",
                mux::pane_end::KEPT
            );
            std::process::exit(NO_END);
        };
        println!("{}", serde_json::to_string(&end)?);
        Ok(())
    }
}
