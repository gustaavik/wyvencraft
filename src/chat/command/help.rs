//! `/help` — list the commands the runner can actually use.

use super::{COMMANDS, ChatCommand, CommandContext, Permission};
use crate::net::ChatKind;

pub struct HelpCommand;

impl ChatCommand for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }

    fn usage(&self) -> &'static str {
        "/help — list the commands you can run"
    }

    fn permission(&self) -> Permission {
        Permission::Everyone
    }

    /// Reads the registry rather than a hand-maintained list, so a command added
    /// to [`COMMANDS`] documents itself here with no edit to this file.
    fn run(&self, _args: &str, ctx: &mut dyn CommandContext) {
        let is_op = ctx.is_op();
        ctx.reply(ChatKind::System, "commands:".to_string());
        for command in COMMANDS {
            if is_op || command.permission() == Permission::Everyone {
                ctx.reply(ChatKind::System, format!("  {}", command.usage()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::FakeContext;
    use super::*;

    fn run(is_op: bool) -> FakeContext {
        let mut ctx = FakeContext::new(is_op, ["bread"]);
        HelpCommand.run("", &mut ctx);
        ctx
    }

    /// Listing a command you'd only be refused is a worse experience than not
    /// listing it, so the output is filtered by what the runner may actually do.
    #[test]
    fn an_op_is_shown_more_than_everyone_else() {
        let ops_only = COMMANDS
            .iter()
            .filter(|c| c.permission() == Permission::Op)
            .count();
        assert!(
            ops_only > 0,
            "this test is meaningless without an op command"
        );

        assert_eq!(run(true).replies.len(), COMMANDS.len() + 1, "header + all");
        assert_eq!(
            run(false).replies.len(),
            COMMANDS.len() - ops_only + 1,
            "op-only commands are hidden"
        );
    }

    #[test]
    fn help_lists_give_for_an_op() {
        assert!(run(true).said(ChatKind::System).contains("/give"));
        assert!(!run(false).said(ChatKind::System).contains("/give"));
    }

    /// `/help` has to be runnable by the people who most need it.
    #[test]
    fn help_is_open_to_everyone() {
        assert_eq!(HelpCommand.permission(), Permission::Everyone);
    }
}
