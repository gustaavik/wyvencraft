//! Commands: one per file, behind one trait, found through one registry.
//!
//! Every command implements [`ChatCommand`] and is listed in [`COMMANDS`], which
//! follows the precedent set by `model::ModelLoader`: **a new command is a new
//! file plus one line here**, and no existing implementation is touched. There is
//! no `Command` enum and no `match` over command kinds, because both would have
//! to be edited for every addition.
//!
//! A command owns its whole grammar. `/give` parses its own arguments in
//! [`give`], reports its own usage errors, and phrases its own feedback — the
//! dispatcher below only decides *which* command was typed and whether the
//! runner is allowed to run it.
//!
//! Commands reach the world through [`CommandContext`], never through the state
//! layer directly; see [`context`] for why.

mod context;
mod fake;
mod give;
mod help;
mod tp;

pub use context::{CommandContext, Position};
pub use fake::FakeContext;
pub use give::GiveCommand;
pub use help::HelpCommand;
pub use tp::TpCommand;

use crate::net::ChatKind;

/// Who may run a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Anyone in the session.
    Everyone,
    /// Only a player listed in `ops.toml` (and the host, always).
    Op,
}

/// One command the player can type.
///
/// Implementors are unit structs held in [`COMMANDS`] as `&'static dyn` — they
/// carry no state, so the registry is a `const` and dispatch needs no allocation.
pub trait ChatCommand: Sync {
    /// The word typed after the slash, matched case-insensitively.
    fn name(&self) -> &'static str;

    /// One-line usage, listed by `/help`.
    fn usage(&self) -> &'static str;

    /// Who is allowed to run it. Checked by the dispatcher *before* [`run`] is
    /// called, so an implementation never has to remember to check.
    ///
    /// [`run`]: ChatCommand::run
    fn permission(&self) -> Permission;

    /// Run it. `args` is everything after the command name, already trimmed;
    /// parsing it is the command's own business.
    fn run(&self, args: &str, ctx: &mut dyn CommandContext);
}

/// Every command this build knows about. **Adding a command: a new module above
/// and one entry here.**
pub const COMMANDS: &[&dyn ChatCommand] = &[&GiveCommand, &HelpCommand, &TpCommand];

/// What one submitted line turned out to be.
pub enum Invocation<'a> {
    /// Not a command — ordinary chat. The `/` prefix is the only distinction,
    /// so saying "help" out loud can never run anything.
    Message,
    /// A known command and its raw arguments.
    Command {
        command: &'static dyn ChatCommand,
        args: &'a str,
    },
    /// A `/…` with no command by that name.
    Unknown { typed: &'a str },
}

/// Match one submitted line against the registry.
pub fn resolve(input: &str) -> Invocation<'_> {
    let Some(body) = input.trim().strip_prefix('/') else {
        return Invocation::Message;
    };
    let (name, args) = match body.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (body, ""),
    };
    match COMMANDS
        .iter()
        .find(|command| command.name().eq_ignore_ascii_case(name))
    {
        Some(command) => Invocation::Command {
            command: *command,
            args,
        },
        None => Invocation::Unknown { typed: name },
    }
}

/// The message shown for a `/…` nobody implements.
pub fn unknown_command_message(typed: &str) -> String {
    format!("unknown command '/{typed}' — try /help")
}

/// The message shown when a command is refused.
pub fn unauthorized_message(command: &str) -> String {
    format!("you are not authorized to use /{command}")
}

/// The nearest candidate to `needle`: a case-insensitive prefix match if there
/// is one, otherwise a substring match. Powers "did you mean …?" on a typo'd
/// argument without pulling in a fuzzy-matching dependency.
pub fn suggest<'a>(needle: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut substring = None;
    for candidate in candidates {
        let lower = candidate.to_ascii_lowercase();
        if lower.starts_with(&needle) {
            return Some(candidate);
        }
        if substring.is_none() && (lower.contains(&needle) || needle.contains(&lower)) {
            substring = Some(candidate);
        }
    }
    substring
}

/// Shared helper: reply with an error and stop. Commands use this for their own
/// usage failures so every refusal reads the same way.
pub(crate) fn refuse(ctx: &mut dyn CommandContext, message: String) {
    ctx.reply(ChatKind::Error, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `/` prefix is the whole distinction between chatting and commanding.
    /// If a bare message ever resolved to a command, saying "help" out loud
    /// would run one.
    #[test]
    fn a_bare_message_is_not_a_command() {
        for line in ["help", "give me a break", "  hello there  "] {
            assert!(
                matches!(resolve(line), Invocation::Message),
                "{line:?} is chat"
            );
        }
    }

    #[test]
    fn a_known_command_resolves_with_its_raw_arguments() {
        let Invocation::Command { command, args } = resolve("/give raw beef 3") else {
            panic!("/give is registered");
        };
        assert_eq!(command.name(), "give");
        assert_eq!(args, "raw beef 3", "the command parses its own arguments");
    }

    #[test]
    fn a_command_without_arguments_resolves_with_an_empty_tail() {
        let Invocation::Command { command, args } = resolve("/help") else {
            panic!("/help is registered");
        };
        assert_eq!(command.name(), "help");
        assert_eq!(args, "");
    }

    #[test]
    fn command_names_are_case_insensitive() {
        assert!(matches!(resolve("/HELP"), Invocation::Command { .. }));
    }

    #[test]
    fn an_unregistered_command_reports_itself() {
        let Invocation::Unknown { typed } = resolve("/weather clear") else {
            panic!("/weather is not registered");
        };
        assert_eq!(typed, "weather");
        assert!(
            unknown_command_message(typed).contains("/help"),
            "points a way out"
        );
    }

    /// The registry is what `/help` prints and what the dispatcher searches, so
    /// duplicate or mis-cased names would silently shadow a command.
    #[test]
    fn every_registered_command_has_a_unique_lowercase_name() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "command names collide");
        assert!(
            COMMANDS
                .iter()
                .all(|c| c.name() == c.name().to_ascii_lowercase()),
            "names are matched case-insensitively; keep them lowercase"
        );
        assert!(
            COMMANDS.iter().all(|c| c.usage().starts_with('/')),
            "a usage line is what /help prints; it should read as a command"
        );
    }

    #[test]
    fn a_near_miss_gets_a_suggestion() {
        let items = ["bread", "raw beef", "wooden pickaxe"];
        assert_eq!(suggest("brea", items), Some("bread"));
        assert_eq!(suggest("BREAD", items), Some("bread"));
        assert_eq!(suggest("beef", items), Some("raw beef"));
        assert_eq!(suggest("zzz", items), None);
        assert_eq!(suggest("", items), None);
    }
}
