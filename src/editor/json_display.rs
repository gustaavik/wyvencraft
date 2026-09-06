//! Reading and writing the `display` block of a Java-model `.json`, in place.
//!
//! Everything here is a pure `&str` → `String`. That matters more than it
//! sounds: these files are hand-tuned, reviewed and committed, so a save has to
//! leave every byte it did not mean to change exactly as it found it. A
//! `serde_json` round trip would reformat all eight `elements` of a sword to
//! move one number, and the diff would be unreadable.
//!
//! So the file is scanned rather than reparsed: the `display` member's byte span
//! is located, its entries are read in the order the file lists them, the edited
//! one is replaced, and the block is re-rendered with the file's own indentation
//! and spliced back over that span. Nothing outside the span is touched.

use wyven_model::display::{DisplayContext, DisplayTransforms, ItemTransform};
use wyven_model::generated;

/// The `display` key of a Minecraft item model, per context.
pub fn context_key(context: DisplayContext) -> &'static str {
    match context {
        DisplayContext::FirstPersonRightHand => "firstperson_righthand",
        DisplayContext::ThirdPersonRightHand => "thirdperson_righthand",
        DisplayContext::Gui => "gui",
        DisplayContext::Ground => "ground",
        DisplayContext::Fixed => "fixed",
        DisplayContext::Head => "head",
    }
}

/// The off-hand twin Blockbench writes beside a hand entry.
///
/// We have no off-hand and the loader ignores these, but every shipped file
/// carries them, and leaving one behind at the old value would confuse the next
/// person to open the model in Blockbench.
fn lefthand_twin(context: DisplayContext) -> Option<&'static str> {
    match context {
        DisplayContext::FirstPersonRightHand => Some("firstperson_lefthand"),
        DisplayContext::ThirdPersonRightHand => Some("thirdperson_lefthand"),
        _ => None,
    }
}

/// The order a seeded block is written in — Blockbench's own.
const SEED_ORDER: [DisplayContext; 6] = [
    DisplayContext::ThirdPersonRightHand,
    DisplayContext::FirstPersonRightHand,
    DisplayContext::Ground,
    DisplayContext::Gui,
    DisplayContext::Head,
    DisplayContext::Fixed,
];

/// The placement this file asks for in `context`, resolved the way the loader
/// resolves it.
///
/// A generated stub with no `display` block of its own is reported with
/// Minecraft's `item/generated` numbers rather than the identity, because those
/// are what it is actually drawn with — see `wyven_model::blockjson`.
pub fn read_display(json: &str, context: DisplayContext) -> Result<ItemTransform, String> {
    Ok(effective_display(json)?.get(context).unwrap_or_default())
}

/// Every placement in force for this file, defaults included.
fn effective_display(json: &str) -> Result<DisplayTransforms, String> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Document {
        parent: Option<String>,
        display: DisplayTransforms,
    }

    let document: Document = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let generated = document
        .parent
        .as_deref()
        .is_some_and(|parent| parent.trim_start_matches("minecraft:") == "item/generated");
    match generated && document.display.is_empty() {
        true => Ok(generated::default_display()),
        false => Ok(document.display),
    }
}

/// Rewrite one `display` entry, leaving the rest of the file byte-identical.
///
/// Writing a single entry into a generated stub that had no `display` block at
/// all seeds the *other* five from `item/generated` first. The loader's fallback
/// is all-or-nothing — a stub with any `display` block keeps only what it
/// declares — so without the seed, nudging an apple in the fist would silently
/// flatten its inventory icon and its placement on the ground.
pub fn write_display(
    json: &str,
    context: DisplayContext,
    value: &ItemTransform,
) -> Result<String, String> {
    let root = object_members(json, 0)?;
    let indent = indent_of(json, &root);
    let inner = indent.repeat(2);

    let existing = root.iter().find(|member| member.key == "display");
    let mut entries: Vec<(String, ItemTransform)> = match existing {
        Some(member) => {
            let members = object_members(json, member.value_start)?;
            members
                .iter()
                .map(|entry| {
                    let text = &json[entry.value_start..entry.value_end];
                    let parsed: ItemTransform =
                        serde_json::from_str(text).map_err(|err| err.to_string())?;
                    Ok((entry.key.clone(), parsed))
                })
                .collect::<Result<_, String>>()?
        }
        None => Vec::new(),
    };

    if entries.is_empty() {
        let seed = effective_display(json)?;
        for context in SEED_ORDER {
            if let Some(transform) = seed.get(context) {
                entries.push((context_key(context).to_string(), transform));
            }
        }
    }

    set_entry(&mut entries, context_key(context), *value);
    // Only mirror a twin the file already carries: adding one to a model that
    // never had it would be inventing content the author did not write.
    if let Some(twin) = lefthand_twin(context)
        && entries.iter().any(|(name, _)| name == twin)
    {
        set_entry(&mut entries, twin, *value);
    }

    let block = render_block(&entries, &indent, &inner);
    Ok(match existing {
        Some(member) => splice(json, member.key_start, member.value_end, &block),
        None => insert_member(json, &root, &indent, &block)?,
    })
}

/// Replace an entry, or append it in the order the caller asked for.
fn set_entry(entries: &mut Vec<(String, ItemTransform)>, key: &str, value: ItemTransform) {
    match entries.iter_mut().find(|(name, _)| name == key) {
        Some(slot) => slot.1 = value,
        None => entries.push((key.to_string(), value)),
    }
}

/// `"display": { .. }`, laid out the way Blockbench lays it out.
fn render_block(entries: &[(String, ItemTransform)], indent: &str, inner: &str) -> String {
    let field = format!("{inner}{indent}");
    let rendered: Vec<String> = entries
        .iter()
        .map(|(key, transform)| {
            let mut lines: Vec<String> = Vec::new();
            if transform.rotation != [0.0; 3] {
                lines.push(format!(
                    "{field}\"rotation\": {}",
                    array(transform.rotation)
                ));
            }
            if transform.translation != [0.0; 3] {
                lines.push(format!(
                    "{field}\"translation\": {}",
                    array(transform.translation)
                ));
            }
            if transform.scale != [1.0; 3] {
                lines.push(format!("{field}\"scale\": {}", array(transform.scale)));
            }
            // An entry that says nothing still has to exist: `{}` is the
            // identity placement, while no entry at all falls back to the
            // `[item.model]` spec. They are different answers.
            match lines.is_empty() {
                true => format!("{inner}\"{key}\": {{}}"),
                false => format!("{inner}\"{key}\": {{\n{}\n{inner}}}", lines.join(",\n")),
            }
        })
        .collect();
    format!("\"display\": {{\n{}\n{indent}}}", rendered.join(",\n"))
}

fn array(values: [f32; 3]) -> String {
    let rendered: Vec<String> = values.iter().copied().map(number).collect();
    format!("[{}]", rendered.join(", "))
}

/// Five decimals, whole numbers without a point — what Blockbench writes.
fn number(value: f32) -> String {
    let rounded = (f64::from(value) * 100_000.0).round() / 100_000.0;
    if rounded == 0.0 {
        // Also catches -0.0, which would otherwise print as "-0".
        return "0".to_string();
    }
    match rounded.fract() == 0.0 {
        true => format!("{}", rounded as i64),
        false => format!("{rounded}"),
    }
}

fn splice(text: &str, start: usize, end: usize, replacement: &str) -> String {
    format!("{}{replacement}{}", &text[..start], &text[end..])
}

/// Add a `display` member after the last one the root object has.
fn insert_member(text: &str, root: &[Member], indent: &str, block: &str) -> Result<String, String> {
    let last = root
        .last()
        .ok_or_else(|| "model file has no members to add `display` beside".to_string())?;
    Ok(splice(
        text,
        last.value_end,
        last.value_end,
        &format!(",\n{indent}{block}"),
    ))
}

/// The whitespace the root object's members are indented by, so a rewritten
/// block matches the file it lands in rather than imposing a house style.
fn indent_of(text: &str, root: &[Member]) -> String {
    let Some(first) = root.first() else {
        return "\t".to_string();
    };
    let line_start = text[..first.key_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let indent = &text[line_start..first.key_start];
    match indent.chars().all(char::is_whitespace) && !indent.is_empty() {
        true => indent.to_string(),
        false => "\t".to_string(),
    }
}

/// One member of a JSON object, located in the source text.
struct Member {
    key: String,
    /// The opening quote of the key — where a replacement starts.
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

/// Every member of the object beginning at or after `from`, in file order.
///
/// A scanner rather than a parse because the byte offsets are the whole point:
/// `serde_json` would hand back the values and lose where they were.
fn object_members(text: &str, from: usize) -> Result<Vec<Member>, String> {
    let bytes = text.as_bytes();
    let mut at = skip_whitespace(bytes, from);
    if bytes.get(at) != Some(&b'{') {
        return Err("expected a JSON object".to_string());
    }
    at += 1;

    let mut members = Vec::new();
    loop {
        at = skip_whitespace(bytes, at);
        match bytes.get(at) {
            Some(b'}') => return Ok(members),
            Some(b',') => {
                at += 1;
                continue;
            }
            Some(b'"') => {}
            _ => return Err("malformed JSON object".to_string()),
        }
        let key_start = at;
        let key_end = scan_string(bytes, at)?;
        at = skip_whitespace(bytes, key_end);
        if bytes.get(at) != Some(&b':') {
            return Err("expected `:` after a JSON key".to_string());
        }
        let value_start = skip_whitespace(bytes, at + 1);
        let value_end = scan_value(bytes, value_start)?;
        members.push(Member {
            key: text[key_start + 1..key_end - 1].to_string(),
            key_start,
            value_start,
            value_end,
        });
        at = value_end;
    }
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        at += 1;
    }
    at
}

/// `at` is the opening quote; returns the index just past the closing one.
fn scan_string(bytes: &[u8], at: usize) -> Result<usize, String> {
    let mut index = at + 1;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err("unterminated JSON string".to_string())
}

/// `at` is the first byte of a value; returns the index just past its last.
fn scan_value(bytes: &[u8], at: usize) -> Result<usize, String> {
    match bytes.get(at) {
        Some(b'"') => scan_string(bytes, at),
        Some(open @ (b'{' | b'[')) => {
            let close = if *open == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            let mut index = at;
            while let Some(&byte) = bytes.get(index) {
                match byte {
                    b'"' => {
                        index = scan_string(bytes, index)?;
                        continue;
                    }
                    b if b == *open => depth += 1,
                    b if b == close => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(index + 1);
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
            Err("unterminated JSON object or array".to_string())
        }
        Some(_) => {
            let mut index = at;
            while let Some(&byte) = bytes.get(index) {
                if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                    break;
                }
                index += 1;
            }
            Ok(index)
        }
        None => Err("JSON value expected".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWORD: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/models/items/wooden_sword.json"
    );
    const APPLE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/models/items/apple.json"
    );

    fn shipped(path: &str) -> String {
        std::fs::read_to_string(path).expect("a shipped model file")
    }

    #[test]
    fn a_declared_placement_is_read_back() {
        let sword = shipped(SWORD);
        let first = read_display(&sword, DisplayContext::FirstPersonRightHand).expect("reads");
        assert_eq!(first.rotation, [-99.9, 87.78, 95.45]);
        assert_eq!(first.translation, [0.0, 1.0, 1.0]);
        assert_eq!(first.scale, [0.79883; 3]);
    }

    /// A three-line stub is drawn with Minecraft's numbers, so that is what the
    /// panel has to open showing — the identity would be a lie.
    #[test]
    fn a_generated_stub_reads_as_its_defaults() {
        let apple = shipped(APPLE);
        let first = read_display(&apple, DisplayContext::FirstPersonRightHand).expect("reads");
        assert_eq!(
            Some(first),
            generated::default_display().get(DisplayContext::FirstPersonRightHand)
        );
    }

    /// The load-bearing promise: a save moves the numbers it was asked to move
    /// and leaves the rest of the file exactly as it found it.
    #[test]
    fn a_save_touches_only_the_display_block() {
        let sword = shipped(SWORD);
        let value = ItemTransform {
            rotation: [-12.5, 0.0, 90.0],
            translation: [0.0, 4.0, 1.0],
            scale: [0.5; 3],
        };
        let written =
            write_display(&sword, DisplayContext::FirstPersonRightHand, &value).expect("writes");

        let before = &sword[..sword.find("\"display\"").expect("has a display block")];
        let after = &written[..written.find("\"display\"").expect("still has one")];
        assert_eq!(before, after, "everything before `display` is untouched");

        let tail = |text: &str| {
            let at = text.rfind("\"groups\"").expect("has groups");
            text[at..].to_string()
        };
        assert_eq!(tail(&sword), tail(&written), "and everything after it");
    }

    #[test]
    fn a_saved_placement_reads_back_as_written() {
        let sword = shipped(SWORD);
        let value = ItemTransform {
            rotation: [-12.5, 0.0, 90.0],
            translation: [0.0, 4.0, 1.0],
            scale: [0.5; 3],
        };
        let written =
            write_display(&sword, DisplayContext::ThirdPersonRightHand, &value).expect("writes");
        assert_eq!(
            read_display(&written, DisplayContext::ThirdPersonRightHand),
            Ok(value)
        );
        assert_eq!(
            read_display(&written, DisplayContext::Gui),
            read_display(&sword, DisplayContext::Gui),
            "an untouched context keeps its value"
        );
    }

    /// Blockbench writes both hands. Leaving the twin behind at the old value
    /// would show the wrong placement the next time the model is opened there.
    #[test]
    fn a_hand_entry_carries_its_lefthand_twin() {
        let sword = shipped(SWORD);
        let value = ItemTransform {
            rotation: [1.0, 2.0, 3.0],
            translation: [4.0, 5.0, 6.0],
            scale: [0.25; 3],
        };
        let written =
            write_display(&sword, DisplayContext::FirstPersonRightHand, &value).expect("writes");

        let block = &written[written.find("\"display\"").expect("block")..];
        let entry = block
            .find("\"firstperson_lefthand\"")
            .map(|at| &block[at..at + 200])
            .expect("the twin is still there");
        assert!(entry.contains("[1, 2, 3]"), "twin follows: {entry}");
    }

    /// A `ground` entry alone has no twin to mirror into, and one must not be
    /// invented.
    #[test]
    fn a_non_hand_entry_gains_no_twin() {
        let sword = shipped(SWORD);
        let written = write_display(&sword, DisplayContext::Ground, &ItemTransform::default())
            .expect("writes");
        assert_eq!(
            written.matches("lefthand").count(),
            sword.matches("lefthand").count()
        );
    }

    /// The loader's generated fallback is all-or-nothing: a stub with *any*
    /// display block keeps only what it declares. So writing one entry has to
    /// write the other five too, or nudging an apple in the fist would flatten
    /// its inventory icon.
    #[test]
    fn writing_into_a_generated_stub_keeps_its_other_contexts() {
        let apple = shipped(APPLE);
        let value = ItemTransform {
            rotation: [0.0, -45.0, 0.0],
            translation: [1.0, 3.0, 1.0],
            scale: [0.7; 3],
        };
        let written =
            write_display(&apple, DisplayContext::FirstPersonRightHand, &value).expect("writes");

        assert_eq!(
            read_display(&written, DisplayContext::FirstPersonRightHand),
            Ok(value)
        );
        for context in [
            DisplayContext::ThirdPersonRightHand,
            DisplayContext::Gui,
            DisplayContext::Ground,
        ] {
            assert_eq!(
                read_display(&written, context).ok(),
                generated::default_display().get(context),
                "{context:?} keeps the placement it was drawn with"
            );
        }
        assert!(
            written.contains("\"parent\""),
            "and the stub's own keys survive"
        );
    }

    /// An all-identity entry is not the same as no entry: the first is the
    /// identity placement, the second falls back to `[item.model]`.
    #[test]
    fn an_identity_entry_is_written_rather_than_omitted() {
        let sword = shipped(SWORD);
        let written =
            write_display(&sword, DisplayContext::Gui, &ItemTransform::default()).expect("writes");
        assert!(written.contains("\"gui\": {}"), "{written}");
        assert_eq!(
            read_display(&written, DisplayContext::Gui),
            Ok(ItemTransform::default())
        );
    }

    #[test]
    fn numbers_are_written_the_way_blockbench_writes_them() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(2.75), "2.75");
        assert_eq!(number(0.79883), "0.79883");
        assert_eq!(number(-83.4), "-83.4");
        // f32 → f64 widening turns 0.68 into 0.6800000071525574; five decimals
        // is what keeps that out of the file.
        assert_eq!(number(0.68), "0.68");
    }

    #[test]
    fn the_file_indentation_is_kept() {
        let spaces = "{\n  \"parent\": \"item/generated\"\n}\n";
        let written =
            write_display(spaces, DisplayContext::Gui, &ItemTransform::default()).expect("writes");
        assert!(written.contains("\n  \"display\": {"), "{written}");
        assert!(!written.contains('\t'), "no tabs introduced: {written}");
    }

    #[test]
    fn a_malformed_file_is_an_error_rather_than_a_rewrite() {
        assert!(write_display("not json", DisplayContext::Gui, &ItemTransform::default()).is_err());
        assert!(read_display("{ oops", DisplayContext::Gui).is_err());
    }

    #[test]
    fn the_scanner_finds_members_past_braces_inside_strings() {
        let text = r#"{ "a": "}{", "display": { "gui": {} }, "b": [1, 2] }"#;
        let members = object_members(text, 0).expect("scans");
        let keys: Vec<&str> = members.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["a", "display", "b"]);
        let display = &members[1];
        assert_eq!(
            &text[display.value_start..display.value_end],
            "{ \"gui\": {} }"
        );
    }
}
