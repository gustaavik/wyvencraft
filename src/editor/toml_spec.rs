//! Reading and writing an item's `[item.model]` numbers in `assets/items.toml`.
//!
//! Reading goes through `toml`, because nothing about the file's shape matters
//! for that. Writing does **not**: `assets/items.toml` opens with sixty lines
//! documenting its own schema and carries a comment beside half its entries, and
//! `toml::to_string` would delete every one of them to move a decimal point. So
//! a save is line surgery — find the item, find its `[item.model]` table, and
//! rewrite only the `scale`, `offset` and `rotation` lines inside it.
//!
//! Pure `&str` → `String` throughout, which is what lets the surgery be pinned
//! against the real shipped file with no filesystem in the test.

use wyven_model::ModelSpec;

use super::placement::SpecPlacement;

/// The `[item.model]` numbers this file gives `item`.
pub fn read_spec(text: &str, item: &str) -> Result<SpecPlacement, String> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        item: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        model: Option<ModelSpec>,
    }

    let file: File = toml::from_str(text).map_err(|err| err.to_string())?;
    let entry = file
        .item
        .into_iter()
        .find(|entry| entry.id == item)
        .ok_or_else(|| format!("no item {item:?} in items.toml"))?;
    let spec = entry
        .model
        .ok_or_else(|| format!("item {item:?} has no [item.model] table"))?;
    Ok(SpecPlacement {
        scale: spec.scale,
        offset: spec.offset,
        rotation: spec.rotation,
    })
}

/// Rewrite `item`'s `[item.model]` numbers, leaving every other byte alone.
///
/// A value that equals its default is written as *no line at all*, and an
/// existing line for it is removed — which is how the file is authored (a
/// `vine_sword` with no rotation, a stub with only a `path`) and keeps a save
/// from growing the file with three lines that say nothing.
pub fn write_spec(text: &str, item: &str, spec: &SpecPlacement) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = model_table(&lines, item)?;

    let mut rewritten: Vec<String> = Vec::with_capacity(lines.len() + 3);
    rewritten.extend(lines[..start].iter().map(|line| (*line).to_string()));

    // The header line, then `path`, then whatever of the three the spec still
    // needs — in the order the file documents them.
    let mut kept: Vec<String> = Vec::new();
    for line in &lines[start..end] {
        match key_of(line) {
            Some("scale" | "offset" | "rotation") => {}
            _ => kept.push((*line).to_string()),
        }
    }
    // Trailing blank lines belong after the values, not between them.
    let mut trailing: Vec<String> = Vec::new();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        trailing.push(kept.pop().expect("just checked"));
    }
    rewritten.extend(kept);
    if spec.scale != 1.0 {
        rewritten.push(format!("scale = {}", number(spec.scale)));
    }
    if spec.offset != [0.0; 3] {
        rewritten.push(format!("offset = {}", array(spec.offset)));
    }
    if spec.rotation != [0.0; 3] {
        rewritten.push(format!("rotation = {}", array(spec.rotation)));
    }
    rewritten.extend(trailing.into_iter().rev());
    rewritten.extend(lines[end..].iter().map(|line| (*line).to_string()));

    let mut out = rewritten.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// The half-open line range of `item`'s `[item.model]` table, header included.
///
/// Line-based rather than a `toml` parse because the *positions* are the whole
/// point: the parser would hand back the values and lose where they were.
fn model_table(lines: &[&str], item: &str) -> Result<(usize, usize), String> {
    let mut in_item = false;
    let mut is_target = false;
    let mut start: Option<usize> = None;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // A comment can hold anything, including something that looks like a
        // key or a header. It never ends a table and never names an item.
        if trimmed.starts_with('#') {
            continue;
        }
        // Inside the table already: the next header of any kind ends it.
        if let Some(begin) = start {
            if trimmed.starts_with('[') {
                return Ok((begin, index));
            }
            continue;
        }
        if trimmed == "[[item]]" {
            in_item = true;
            is_target = false;
            continue;
        }
        if in_item && let Some(value) = string_value(trimmed, "id") {
            is_target = value == item;
            continue;
        }
        if is_target && trimmed == "[item.model]" {
            start = Some(index);
        }
    }

    // The last item in the file has no following header to stop at.
    match start {
        Some(start) => Ok((start, lines.len())),
        None => Err(format!("no [item.model] table for item {item:?}")),
    }
}

/// The bare key of a `key = value` line, ignoring comments and blanks.
fn key_of(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return None;
    }
    let key = trimmed.split_once('=')?.0.trim();
    (!key.is_empty()).then_some(key)
}

/// The string a `key = "value"` line assigns, if this is that line.
fn string_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (found, value) = line.split_once('=')?;
    (found.trim() == key).then(|| value.trim().trim_matches('"'))
}

/// TOML floats keep their point: `1` would deserialize as an integer and
/// `ModelSpec` asks for an `f32`.
fn number(value: f32) -> String {
    let rounded = (f64::from(value) * 100_000.0).round() / 100_000.0;
    match rounded.fract() == 0.0 {
        true => format!("{:.1}", rounded),
        false => format!("{rounded}"),
    }
}

fn array(values: [f32; 3]) -> String {
    let rendered: Vec<String> = values.iter().copied().map(number).collect();
    format!("[{}]", rendered.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEMS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/items.toml");

    fn shipped() -> String {
        std::fs::read_to_string(ITEMS).expect("the shipped items.toml")
    }

    #[test]
    fn a_declared_spec_is_read_back() {
        let spec = read_spec(&shipped(), "wooden_pickaxe").expect("reads");
        assert_eq!(spec.scale, 0.55);
        assert_eq!(spec.offset, [-0.5, 0.0, -0.5]);
        assert_eq!(spec.rotation, [0.0, 90.0, 0.0]);
    }

    /// `vine_sword` omits `rotation`, so the default has to come back rather
    /// than an error.
    #[test]
    fn an_omitted_value_reads_as_its_default() {
        let spec = read_spec(&shipped(), "vine_sword").expect("reads");
        assert_eq!(spec.scale, 0.35);
        assert_eq!(spec.rotation, [0.0; 3]);
    }

    #[test]
    fn an_unknown_item_is_an_error() {
        assert!(read_spec(&shipped(), "no_such_item").is_err());
        assert!(read_spec(&shipped(), "apple").is_ok());
    }

    /// The load-bearing promise: the file is 60 lines of schema documentation
    /// and a comment beside half its entries, and a save keeps all of it.
    #[test]
    fn a_save_changes_only_the_targeted_lines() {
        let items = shipped();
        let spec = SpecPlacement {
            scale: 0.6,
            offset: [-0.5, 0.25, -0.5],
            rotation: [0.0, 45.0, 0.0],
        };
        let written = write_spec(&items, "wooden_pickaxe", &spec).expect("writes");

        assert_eq!(
            items.lines().filter(|l| l.starts_with('#')).count(),
            written.lines().filter(|l| l.starts_with('#')).count(),
            "every comment survives"
        );
        assert_eq!(
            read_spec(&written, "wooden_pickaxe"),
            Ok(spec),
            "and the new numbers are there"
        );
        for other in ["wooden_axe", "vine_sword", "stone_sword", "apple"] {
            assert_eq!(
                read_spec(&written, other),
                read_spec(&items, other),
                "{other} is untouched"
            );
        }
    }

    /// A diff no bigger than the edit — three lines in, three lines out.
    #[test]
    fn a_save_of_the_same_values_is_a_no_op() {
        let items = shipped();
        let spec = read_spec(&items, "wooden_sword").expect("reads");
        let written = write_spec(&items, "wooden_sword", &spec).expect("writes");
        assert_eq!(items, written);
    }

    /// `vine_sword` has no `rotation` line; giving it one must insert rather
    /// than fail, and must land inside its own table.
    #[test]
    fn a_missing_value_is_inserted() {
        let items = shipped();
        let spec = SpecPlacement {
            scale: 0.35,
            offset: [-0.5, 0.75, -0.5],
            rotation: [10.0, 0.0, 0.0],
        };
        let written = write_spec(&items, "vine_sword", &spec).expect("writes");
        assert_eq!(read_spec(&written, "vine_sword"), Ok(spec));
        assert_eq!(
            read_spec(&written, "wooden_sword"),
            read_spec(&items, "wooden_sword"),
            "the next item did not absorb the new line"
        );
    }

    /// Back to the default and the line goes away, the way the file authors it.
    #[test]
    fn a_defaulted_value_is_removed_rather_than_written_out() {
        let items = shipped();
        let spec = SpecPlacement {
            scale: 1.0,
            offset: [0.0; 3],
            rotation: [0.0; 3],
        };
        let written = write_spec(&items, "wooden_sword", &spec).expect("writes");
        assert_eq!(read_spec(&written, "wooden_sword"), Ok(spec));

        let table = written
            .split("[[item]]")
            .find(|block| block.contains("id = \"wooden_sword\""))
            .expect("the block");
        assert!(!table.contains("scale ="), "{table}");
        assert!(!table.contains("rotation ="), "{table}");
        assert!(table.contains("path ="), "the path stays: {table}");
    }

    /// An item with a `[item.model]` at the very end of the file has no
    /// following header to stop at.
    #[test]
    fn a_table_at_the_end_of_the_file_is_handled() {
        let text = "[[item]]\nid = \"a\"\n\n[item.model]\npath = \"p\"\nscale = 0.5\n";
        let written = write_spec(
            text,
            "a",
            &SpecPlacement {
                scale: 0.25,
                offset: [0.0; 3],
                rotation: [0.0; 3],
            },
        )
        .expect("writes");
        assert!(written.ends_with("scale = 0.25\n"), "{written:?}");
    }

    #[test]
    fn an_item_without_a_model_table_is_an_error_rather_than_a_rewrite() {
        let text = "[[item]]\nid = \"a\"\n\n[[item]]\nid = \"b\"\n\n[item.model]\npath = \"p\"\n";
        assert!(write_spec(text, "a", &SpecPlacement::default()).is_err());
    }

    #[test]
    fn floats_keep_their_point() {
        assert_eq!(number(1.0), "1.0");
        assert_eq!(number(0.55), "0.55");
        assert_eq!(number(-0.5), "-0.5");
        assert_eq!(array([0.0, 90.0, 0.0]), "[0.0, 90.0, 0.0]");
    }
}
