use crate::{app::App, command::Command};

pub struct BetasCommand;

impl Command for BetasCommand {
    fn name(&self) -> &str {
        "betas"
    }

    fn description(&self, _lc: &crate::i18n::LcRegistry) -> String {
        "\u{6253}\u{5f00} Beta \u{529f}\u{80fd}\u{5f00}\u{5173}\u{9762}\u{677f}".to_string()
    }

    fn execute(&self, app: &mut App, _args: &str) {
        app.open_betas_panel();
    }
}
