//! `/tp <x> <y> <z>` / `/tp <player>` — move the runner somewhere else.

use super::{ChatCommand, CommandContext, Permission, Position, refuse, suggest};
use crate::core::CHUNK_HEIGHT;
use crate::net::ChatKind;

/// How far out along x/z a teleport may go. Well beyond any reachable terrain,
/// and short of where `f32` world coordinates start losing sub-block precision.
pub const MAX_HORIZONTAL: f32 = 1_000_000.0;

const USAGE: &str = "/tp <x> <y> <z> | /tp <player> — teleport (~ = relative)";

pub struct TpCommand;

impl ChatCommand for TpCommand {
    fn name(&self) -> &'static str {
        "tp"
    }

    fn usage(&self) -> &'static str {
        USAGE
    }

    fn permission(&self) -> Permission {
        Permission::Op
    }

    fn run(&self, args: &str, ctx: &mut dyn CommandContext) {
        let destination = match parse_args(args) {
            Ok(destination) => destination,
            Err(message) => return refuse(ctx, message),
        };

        let (position, arrival) = match destination {
            Destination::Coords(coords) => {
                let base = ctx.position();
                let position = [
                    coords[0].resolve(base[0]),
                    coords[1].resolve(base[1]),
                    coords[2].resolve(base[2]),
                ];
                match check_in_world(position) {
                    Ok(()) => (position, describe(position)),
                    Err(message) => return refuse(ctx, message),
                }
            }
            Destination::Player(name) => {
                let players = ctx.player_positions();
                let Some((canonical, position)) = players
                    .iter()
                    .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&name))
                else {
                    return refuse(ctx, unknown_player_message(&name, &players));
                };
                (*position, format!("{canonical} ({})", describe(*position)))
            }
        };

        ctx.teleport(position);
        ctx.reply(ChatKind::System, format!("teleported to {arrival}"));
    }
}

/// Where `/tp` was asked to go.
#[derive(Debug, PartialEq)]
enum Destination {
    /// Three coordinates, each absolute or relative to where the runner is.
    Coords([Coord; 3]),
    /// Another player, by name.
    Player(String),
}

/// One coordinate of a `/tp` destination.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Coord {
    Absolute(f32),
    /// An offset from the runner's current position on this axis (`~`, `~-3`).
    Relative(f32),
}

impl Coord {
    fn resolve(self, base: f32) -> f32 {
        match self {
            Coord::Absolute(value) => value,
            Coord::Relative(offset) => base + offset,
        }
    }
}

/// Parse `<x> <y> <z>` or `<player>`.
///
/// Three tokens that all read as coordinates are a position; anything else is a
/// player name, joined back together because generated names contain a space
/// ("Player 1"). No player can be called "10 64 -3", so the two never collide.
fn parse_args(rest: &str) -> Result<Destination, String> {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(format!("usage: {USAGE}"));
    }

    if tokens.len() == 3
        && let (Some(x), Some(y), Some(z)) = (
            parse_coord(tokens[0]),
            parse_coord(tokens[1]),
            parse_coord(tokens[2]),
        )
    {
        return Ok(Destination::Coords([x, y, z]));
    }

    // Three tokens that *look* numeric but didn't parse are a typo'd position,
    // not a player — saying "no player called 10 nan -3" would be baffling.
    if tokens.len() == 3 && tokens.iter().any(|token| looks_numeric(token)) {
        return Err(format!("usage: {USAGE}"));
    }

    Ok(Destination::Player(tokens.join(" ")))
}

/// `100`, `-3.5`, `~`, `~5`, `~-3`.
fn parse_coord(token: &str) -> Option<Coord> {
    let coord = match token.strip_prefix('~') {
        Some("") => Coord::Relative(0.0),
        Some(offset) => Coord::Relative(finite(offset)?),
        None => Coord::Absolute(finite(token)?),
    };
    Some(coord)
}

/// Parse a finite `f32`. Rejects `inf`/`NaN`, which would otherwise sail through
/// the bounds check below and poison the player's position.
fn finite(text: &str) -> Option<f32> {
    text.parse::<f32>().ok().filter(|value| value.is_finite())
}

fn looks_numeric(token: &str) -> bool {
    token.starts_with(['~', '-', '+', '.']) || token.starts_with(|c: char| c.is_ascii_digit())
}

/// Reject a destination outside the world. Relative coordinates make this
/// reachable by accident (`/tp ~ ~99999 ~`), and a player below bedrock or past
/// the float-precision horizon is stuck.
fn check_in_world(position: Position) -> Result<(), String> {
    if !position.iter().all(|value| value.is_finite()) {
        return Err("that is not a position".to_string());
    }
    let [x, y, z] = position;
    if !(0.0..=CHUNK_HEIGHT as f32).contains(&y) {
        return Err(format!(
            "y must be between 0 and {CHUNK_HEIGHT}, not {y:.1}"
        ));
    }
    if x.abs() > MAX_HORIZONTAL || z.abs() > MAX_HORIZONTAL {
        return Err(format!(
            "x and z must be within {MAX_HORIZONTAL:.0} blocks of the origin"
        ));
    }
    Ok(())
}

fn describe(position: Position) -> String {
    format!("{:.1} {:.1} {:.1}", position[0], position[1], position[2])
}

fn unknown_player_message(typed: &str, players: &[(String, Position)]) -> String {
    if players.is_empty() {
        return format!("no player called '{typed}' — nobody else is here");
    }
    let names = players.iter().map(|(name, _)| name.as_str());
    match suggest(typed, names) {
        Some(near) => format!("no player called '{typed}' — did you mean '{near}'?"),
        None => format!("no player called '{typed}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::FakeContext;
    use super::*;

    /// A runner at a known spot, with one other player to teleport to.
    fn context() -> FakeContext {
        FakeContext::new(true, ["bread"])
            .at([10.0, 64.0, -20.0])
            .with_players([("Player 1", [100.0, 70.0, 200.0])])
    }

    fn run(line: &str) -> FakeContext {
        let mut ctx = context();
        TpCommand.run(line, &mut ctx);
        ctx
    }

    #[test]
    fn absolute_coordinates_move_the_runner_there() {
        let ctx = run("1 2 3");
        assert_eq!(ctx.teleports, [[1.0, 2.0, 3.0]]);
        assert!(ctx.said(ChatKind::System).contains("1.0 2.0 3.0"));
    }

    #[test]
    fn coordinates_may_be_negative_or_fractional() {
        assert_eq!(run("-4.5 12 -0.25").teleports, [[-4.5, 12.0, -0.25]]);
    }

    /// `~` is what makes `/tp` useful without reading the F3 overlay first:
    /// "up 50 from wherever I am" is the common case.
    #[test]
    fn a_bare_tilde_keeps_that_axis_where_it_is() {
        assert_eq!(run("~ ~ ~").teleports, [[10.0, 64.0, -20.0]]);
    }

    #[test]
    fn a_tilde_offset_is_relative_to_the_runner() {
        assert_eq!(run("~ ~50 ~").teleports, [[10.0, 114.0, -20.0]]);
        assert_eq!(run("~-5 ~ ~2.5").teleports, [[5.0, 64.0, -17.5]]);
    }

    #[test]
    fn absolute_and_relative_coordinates_mix() {
        assert_eq!(run("0 ~10 0").teleports, [[0.0, 74.0, 0.0]]);
    }

    #[test]
    fn a_player_name_teleports_to_them() {
        let ctx = run("Player 1");
        assert_eq!(ctx.teleports, [[100.0, 70.0, 200.0]]);
        assert!(ctx.said(ChatKind::System).contains("Player 1"));
    }

    /// Generated names contain a space ("Player 1"), so the name is every token
    /// joined back together, not just the first.
    #[test]
    fn a_multi_word_player_name_survives_parsing() {
        assert_eq!(
            parse_args("Player 1"),
            Ok(Destination::Player("Player 1".to_string()))
        );
    }

    #[test]
    fn player_names_are_matched_case_insensitively() {
        assert_eq!(run("player 1").teleports, [[100.0, 70.0, 200.0]]);
    }

    /// Typing the start of a name is the common near miss, and `suggest` does
    /// prefix matching, so it recovers.
    #[test]
    fn a_partial_player_name_gets_a_suggestion_and_no_teleport() {
        let ctx = run("Player");
        assert!(ctx.teleports.is_empty(), "a suggestion is not a teleport");
        assert!(
            ctx.said(ChatKind::Error)
                .contains("did you mean 'Player 1'"),
            "got {:?}",
            ctx.replies
        );
    }

    /// `suggest` is prefix-then-substring, not edit distance: a transposition
    /// finds nothing, and the message says so plainly rather than guessing.
    #[test]
    fn a_name_with_no_prefix_or_substring_match_is_reported_plainly() {
        let ctx = run("Playr 1");
        assert!(ctx.teleports.is_empty());
        let said = ctx.said(ChatKind::Error);
        assert!(said.contains("no player called 'Playr 1'"), "got {said:?}");
        assert!(!said.contains("did you mean"), "got {said:?}");
    }

    /// In singleplayer there is nobody to go to, and "did you mean ...?" with no
    /// candidates reads like a bug.
    #[test]
    fn an_empty_session_says_so_rather_than_suggesting_nothing() {
        let mut ctx = FakeContext::new(true, ["bread"]);
        TpCommand.run("Steve", &mut ctx);
        assert!(ctx.teleports.is_empty());
        assert!(ctx.said(ChatKind::Error).contains("nobody else is here"));
    }

    /// Relative coordinates make an out-of-world destination easy to reach by
    /// accident; landing under bedrock or past the float horizon strands you.
    #[test]
    fn a_destination_outside_the_world_is_refused() {
        for line in ["~ ~99999 ~", "0 -1 0", "0 300 0", "9999999 64 0"] {
            let ctx = run(line);
            assert!(ctx.teleports.is_empty(), "{line} should be refused");
            assert_eq!(ctx.replies.len(), 1);
            assert_eq!(ctx.replies[0].0, ChatKind::Error);
        }
    }

    /// The world's floor and ceiling are themselves valid.
    #[test]
    fn the_world_bounds_are_inclusive() {
        assert_eq!(run("0 0 0").teleports, [[0.0, 0.0, 0.0]]);
        assert_eq!(
            run(&format!("0 {CHUNK_HEIGHT} 0")).teleports,
            [[0.0, CHUNK_HEIGHT as f32, 0.0]]
        );
    }

    /// `NaN`/`inf` parse as `f32` but would sail through a naive range check and
    /// poison the player's position beyond recovery.
    #[test]
    fn non_finite_coordinates_are_rejected() {
        for line in ["nan 64 0", "inf 64 0", "0 64 -inf", "~inf ~ ~"] {
            let ctx = run(line);
            assert!(ctx.teleports.is_empty(), "{line} should be refused");
        }
    }

    /// A typo'd number must not be reported as a missing player — that error
    /// sends you looking for the wrong problem.
    #[test]
    fn a_malformed_coordinate_reports_the_usage_not_a_missing_player() {
        let ctx = run("10 6four -3");
        assert!(ctx.teleports.is_empty());
        let said = ctx.said(ChatKind::Error);
        assert!(said.contains("usage:"), "got {said:?}");
        assert!(!said.contains("no player"), "got {said:?}");
    }

    #[test]
    fn no_arguments_reports_the_usage() {
        let ctx = run("");
        assert!(ctx.teleports.is_empty());
        assert!(ctx.said(ChatKind::Error).contains("usage:"));
    }

    /// Teleporting is a cheat like `/give`; if this flipped, any connected
    /// player could walk through walls by hopping past them.
    #[test]
    fn tp_is_op_only() {
        assert_eq!(TpCommand.permission(), Permission::Op);
    }
}
