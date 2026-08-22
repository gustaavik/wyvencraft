//! `/give <item> [count]` — put items in the runner's inventory.

use super::{ChatCommand, CommandContext, ItemName, Permission, refuse, suggest};
use crate::net::ChatKind;

/// Largest count `/give` accepts. Big enough to fill an inventory several times
/// over, small enough that a typo'd number can't allocate wildly.
pub const MAX_COUNT: u32 = 1024;

const USAGE: &str = "/give <item> [count] — put items in your inventory";

pub struct GiveCommand;

impl ChatCommand for GiveCommand {
    fn name(&self) -> &'static str {
        "give"
    }

    fn usage(&self) -> &'static str {
        USAGE
    }

    fn permission(&self) -> Permission {
        Permission::Op
    }

    fn run(&self, args: &str, ctx: &mut dyn CommandContext) {
        let Args { item, count } = match parse_args(args) {
            Ok(args) => args,
            Err(message) => return refuse(ctx, message),
        };

        // Resolve against the session's registry rather than trusting the typed
        // spelling: matching case-insensitively means `/give Bread` works.
        let items = ctx.item_names();
        let Some(found) = items.iter().find(|i| i.id.eq_ignore_ascii_case(&item)) else {
            return refuse(ctx, unknown_item_message(&item, &items));
        };

        // Confirm with the display name, not the id: the chat log is for the
        // player, and "gave 5 × Wooden Pickaxe" is what they asked for.
        let message = format!("gave {count} × {}", found.display);
        let id = found.id.clone();
        ctx.give_item(&id, count);
        ctx.reply(ChatKind::System, message);
    }
}

/// Parsed `/give` arguments.
struct Args {
    item: String,
    count: u32,
}

/// Parse `<item> [count]`.
///
/// An id is a single token (`cooked_beef`), so `/give cooked_beef 5` is the ordinary
/// form. Spaces are still folded to underscores, because "Cooked Beef" is how the
/// item reads in the inventory and typing what you see should work. That is why
/// the *last* token is what becomes the count — and only when it looks like a
/// number and something is left over to name.
fn parse_args(rest: &str) -> Result<Args, String> {
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(format!("usage: {USAGE}"));
    }

    let mut count = 1;
    if tokens.len() > 1 && tokens[tokens.len() - 1].parse::<u64>().is_ok() {
        let typed = tokens.pop().expect("checked non-empty");
        match typed.parse::<u32>() {
            Ok(n) if (1..=MAX_COUNT).contains(&n) => count = n,
            _ => {
                return Err(format!(
                    "'{typed}' is not a count between 1 and {MAX_COUNT}"
                ));
            }
        }
    }

    let item = tokens.join("_");
    if item.is_empty() {
        return Err(format!("usage: {USAGE}"));
    }
    Ok(Args { item, count })
}

/// "unknown item 'brad' — did you mean 'bread'?", when there is a near miss.
///
/// Suggests an *id*, because an id is what the player has to type next.
fn unknown_item_message(typed: &str, items: &[ItemName]) -> String {
    match suggest(typed, items.iter().map(|i| i.id.as_str())) {
        Some(near) => format!("unknown item '{typed}' — did you mean '{near}'?"),
        None => format!("unknown item '{typed}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::FakeContext;
    use super::*;

    fn run(line: &str) -> FakeContext {
        let mut ctx = FakeContext::new(true, ["bread", "cooked_beef", "wooden_pickaxe"]);
        GiveCommand.run(line, &mut ctx);
        ctx
    }

    #[test]
    fn give_defaults_to_one_item() {
        assert_eq!(run("bread").given, [("bread".to_string(), 1)]);
    }

    #[test]
    fn give_takes_a_trailing_count() {
        assert_eq!(run("bread 12").given, [("bread".to_string(), 12)]);
    }

    /// An id is one token, so `/give cooked_beef 3` is the plain form.
    #[test]
    fn an_id_is_a_single_token() {
        assert_eq!(run("cooked_beef 3").given, [("cooked_beef".to_string(), 3)]);
        assert_eq!(
            run("wooden_pickaxe").given,
            [("wooden_pickaxe".to_string(), 1)]
        );
    }

    /// Typing the label you see in the inventory works too: spaces fold to
    /// underscores, so the name cannot be the first token — it is everything
    /// before the count.
    #[test]
    fn spaces_fold_to_underscores_so_the_displayed_name_also_works() {
        assert_eq!(run("cooked beef 3").given, [("cooked_beef".to_string(), 3)]);
    }

    /// The registry holds the canonical id, so a differently-cased request still
    /// resolves — and the confirmation echoes the *display name*, not the id.
    #[test]
    fn ids_are_matched_case_insensitively() {
        let ctx = run("COOKED BEEF 2");
        assert_eq!(ctx.given, [("cooked_beef".to_string(), 2)]);
        assert!(
            ctx.said(ChatKind::System).contains("Cooked Beef"),
            "got {:?}",
            ctx.replies
        );
    }

    /// The chat log is for the player: the reply must quote the label, never
    /// the underscored key it was resolved through.
    #[test]
    fn the_confirmation_quotes_the_display_name_not_the_id() {
        let said = run("wooden_pickaxe 2").said(ChatKind::System);
        assert!(said.contains("Wooden Pickaxe"), "got {said:?}");
        assert!(!said.contains("wooden_pickaxe"), "leaked the id: {said:?}");
    }

    /// An item whose id *is* a number must still be nameable, which is why a
    /// lone token is never treated as the count.
    #[test]
    fn a_lone_token_is_always_the_item_name() {
        let mut ctx = FakeContext::new(true, ["42"]);
        GiveCommand.run("42", &mut ctx);
        assert_eq!(ctx.given, [("42".to_string(), 1)]);
    }

    #[test]
    fn no_arguments_reports_the_usage() {
        let ctx = run("");
        assert!(ctx.given.is_empty());
        assert!(ctx.said(ChatKind::Error).contains("/give <item>"));
    }

    /// A count is validated, not clamped: silently turning `/give bread 0` into
    /// one loaf hides the typo, and an unbounded count would let a slip allocate
    /// thousands of stacks.
    #[test]
    fn an_out_of_range_count_is_rejected_and_gives_nothing() {
        for line in ["bread 0", "bread 99999"] {
            let ctx = run(line);
            assert!(ctx.given.is_empty(), "{line} should give nothing");
            assert!(ctx.said(ChatKind::Error).contains("not a count"));
        }
    }

    #[test]
    fn a_typo_gets_a_suggestion_and_gives_nothing() {
        let ctx = run("brea 5");
        assert!(ctx.given.is_empty());
        assert!(
            ctx.said(ChatKind::Error).contains("did you mean 'bread'"),
            "got {:?}",
            ctx.replies
        );
    }

    #[test]
    fn an_item_with_no_near_miss_is_reported_plainly() {
        let ctx = run("zzzz");
        assert!(ctx.given.is_empty());
        assert!(ctx.said(ChatKind::Error).contains("unknown item 'zzzz'"));
    }

    /// The dispatcher gates on this before `run` is ever called; if it flipped
    /// to `Everyone`, every connected player could spawn anything.
    #[test]
    fn give_is_op_only() {
        assert_eq!(GiveCommand.permission(), Permission::Op);
    }
}
