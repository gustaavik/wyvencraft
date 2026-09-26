//! `/progress` — read or rewrite how far this world is through the boss loop.
//!
//! - `/progress` — what is revealed and who has fallen
//! - `/progress reveal <structure>` — reveal the nearest one, as a shrine would
//! - `/progress defeat <boss>` — mark a boss beaten
//! - `/progress reset` — forget it all
//!
//! Op-only: this is a testing and server-admin tool, not a way to play.

use super::{ChatCommand, CommandContext, Permission, suggest};
use crate::net::ChatKind;

pub struct ProgressCommand;

impl ChatCommand for ProgressCommand {
    fn name(&self) -> &'static str {
        "progress"
    }

    fn usage(&self) -> &'static str {
        "/progress [reveal <structure> | defeat <boss> | reset]"
    }

    fn permission(&self) -> Permission {
        Permission::Op
    }

    fn run(&self, args: &str, ctx: &mut dyn CommandContext) {
        let (verb, rest) = match args.trim().split_once(char::is_whitespace) {
            Some((verb, rest)) => (verb, rest.trim()),
            None => (args.trim(), ""),
        };
        match verb {
            "" => report(ctx),
            "reveal" => reveal(rest, ctx),
            "defeat" => defeat(rest, ctx),
            "reset" => {
                ctx.reset_progression();
                ctx.reply(ChatKind::System, "progression reset".into());
            }
            other => ctx.reply(
                ChatKind::Error,
                format!("unknown '{other}' — usage: {}", self.usage()),
            ),
        }
    }
}

fn report(ctx: &mut dyn CommandContext) {
    let progress = ctx.progression();
    let revealed: Vec<String> = progress
        .revealed()
        .map(|(id, at)| format!("{id} at {} {} {}", at.x, at.y, at.z))
        .collect();
    let defeated: Vec<&str> = progress.defeated.iter().map(String::as_str).collect();
    let list = |items: &[String]| {
        if items.is_empty() {
            "nothing".to_string()
        } else {
            items.join(", ")
        }
    };
    let defeated: Vec<String> = defeated.iter().map(|s| (*s).to_string()).collect();
    ctx.reply(
        ChatKind::System,
        format!(
            "shrines read: {} · revealed: {} · defeated: {}",
            progress.read_shrines.len(),
            list(&revealed),
            list(&defeated)
        ),
    );
}

fn reveal(structure: &str, ctx: &mut dyn CommandContext) {
    let ids = ctx.structure_ids();
    if !ids.iter().any(|id| id == structure) {
        return unknown("structure", structure, &ids, ctx);
    }
    match ctx.reveal(structure) {
        Some([x, y, z]) => ctx.reply(
            ChatKind::System,
            format!("revealed {structure} at {x:.0} {y:.0} {z:.0}"),
        ),
        None => ctx.reply(ChatKind::System, format!("no {structure} anywhere near")),
    }
}

fn defeat(boss: &str, ctx: &mut dyn CommandContext) {
    let ids = ctx.boss_ids();
    if !ids.iter().any(|id| id == boss) {
        return unknown("boss", boss, &ids, ctx);
    }
    let text = if ctx.defeat_boss(boss) {
        format!("{boss} marked defeated")
    } else {
        format!("{boss} was already defeated")
    };
    ctx.reply(ChatKind::System, text);
}

fn unknown(what: &str, name: &str, known: &[String], ctx: &mut dyn CommandContext) {
    let hint = suggest(name, known.iter().map(String::as_str))
        .map(|s| format!(" — did you mean '{s}'?"))
        .unwrap_or_else(|| format!(" — one of: {}", known.join(", ")));
    ctx.reply(ChatKind::Error, format!("no {what} '{name}'{hint}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::command::FakeContext;

    fn ctx() -> FakeContext {
        let mut ctx = FakeContext::new(true, []);
        ctx.structures = vec![("meadows_altar".into(), [300.5, 96.0, -399.5])];
        ctx.bosses = vec!["elder stag".into()];
        ctx
    }

    #[test]
    fn reveal_then_report() {
        let mut ctx = ctx();
        ProgressCommand.run("reveal meadows_altar", &mut ctx);
        assert_eq!(ctx.progress.revealed().count(), 1);
        ProgressCommand.run("", &mut ctx);
        assert!(
            ctx.said(ChatKind::System)
                .contains("meadows_altar at 300 96 -399")
        );
    }

    /// Boss names have spaces; everything after the verb is the name.
    #[test]
    fn defeat_takes_a_spaced_name() {
        let mut ctx = ctx();
        ProgressCommand.run("defeat elder stag", &mut ctx);
        assert!(ctx.progress.is_defeated("elder stag"));
        ProgressCommand.run("defeat elder stag", &mut ctx);
        assert!(ctx.said(ChatKind::System).contains("already"));
    }

    #[test]
    fn reset_forgets_and_unknowns_are_refused() {
        let mut ctx = ctx();
        ProgressCommand.run("defeat elder stag", &mut ctx);
        ProgressCommand.run("reset", &mut ctx);
        assert!(!ctx.progress.is_defeated("elder stag"));
        ProgressCommand.run("defeat elder", &mut ctx);
        ProgressCommand.run("summon", &mut ctx);
        let errors = ctx.said(ChatKind::Error);
        assert!(errors.contains("did you mean 'elder stag'"), "{errors}");
        assert!(errors.contains("unknown 'summon'"), "{errors}");
    }
}
