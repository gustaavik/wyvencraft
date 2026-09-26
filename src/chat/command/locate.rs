//! `/locate <structure>` — where the nearest shrine or altar of a kind stands.
//!
//! An op's tool for testing the progression loop: it answers from the seed,
//! reveals nothing, and pairs with `/tp` to go and look.

use super::{ChatCommand, CommandContext, Permission, suggest};
use crate::net::ChatKind;

pub struct LocateCommand;

impl ChatCommand for LocateCommand {
    fn name(&self) -> &'static str {
        "locate"
    }

    fn usage(&self) -> &'static str {
        "/locate <structure> — where the nearest one stands"
    }

    fn permission(&self) -> Permission {
        Permission::Op
    }

    fn run(&self, args: &str, ctx: &mut dyn CommandContext) {
        let ids = ctx.structure_ids();
        let wanted = args.trim();
        if wanted.is_empty() {
            let text = format!("usage: {} — one of: {}", self.usage(), ids.join(", "));
            return ctx.reply(ChatKind::Error, text);
        }
        if !ids.iter().any(|id| id == wanted) {
            let hint = suggest(wanted, ids.iter().map(String::as_str))
                .map(|s| format!(" — did you mean '{s}'?"))
                .unwrap_or_default();
            return ctx.reply(ChatKind::Error, format!("no structure '{wanted}'{hint}"));
        }
        match ctx.locate(wanted) {
            Some([x, y, z]) => {
                let here = ctx.position();
                let distance = (x - here[0]).hypot(z - here[2]);
                let text = format!("{wanted}: {x:.0} {y:.0} {z:.0} ({distance:.0} blocks away)");
                ctx.reply(ChatKind::System, text);
            }
            None => ctx.reply(ChatKind::System, format!("no {wanted} anywhere near")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::command::FakeContext;

    fn ctx() -> FakeContext {
        let mut ctx = FakeContext::new(true, []).at([0.0, 90.0, 0.0]);
        ctx.structures = vec![("meadows_altar".into(), [300.5, 96.0, -399.5])];
        ctx
    }

    #[test]
    fn it_names_the_nearest_site_and_how_far() {
        let mut ctx = ctx();
        LocateCommand.run("meadows_altar", &mut ctx);
        let said = ctx.said(ChatKind::System);
        assert!(said.contains("300 96 -400"), "{said}");
        assert!(said.contains("500 blocks"), "{said}");
        assert!(
            ctx.progress.revealed().next().is_none(),
            "locating reveals nothing"
        );
    }

    #[test]
    fn a_typo_gets_a_suggestion() {
        let mut ctx = ctx();
        LocateCommand.run("meadows_alt", &mut ctx);
        assert!(
            ctx.said(ChatKind::Error)
                .contains("did you mean 'meadows_altar'")
        );
    }

    #[test]
    fn no_argument_lists_the_structures() {
        let mut ctx = ctx();
        LocateCommand.run("", &mut ctx);
        assert!(ctx.said(ChatKind::Error).contains("meadows_altar"));
    }
}
