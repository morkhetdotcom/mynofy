//! A deliberately small configuration profile on a maintained YAML event parser.
//! No custom YAML scanner, tag construction, alias expansion or merge semantics.
use crate::BoxError;
use saphyr_parser::{Event, Parser, ScalarStyle, Span, StrInput};
use serde_json::{Map, Value};
use std::io::{self, Write};

pub(super) const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 1_000_000;
struct Reader<'a> {
    parser: Parser<'a, StrInput<'a>>,
    events: usize,
}
impl<'a> Reader<'a> {
    fn next(&mut self) -> Result<(Event<'a>, Span), BoxError> {
        self.events += 1;
        if self.events > MAX_EVENTS {
            return Err("YAML configuration exceeds event budget".into());
        }
        self.parser
            .next_event()
            .ok_or_else(|| BoxError::from("Unexpected end of YAML"))?
            .map_err(Into::into)
    }
    fn error(span: Span, message: &str) -> BoxError {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "YAML line {}, column {}: {message}",
                span.start.line(),
                span.start.col() + 1
            ),
        )
        .into()
    }
    fn node(&mut self, event: Event<'a>, span: Span, depth: usize) -> Result<Value, BoxError> {
        if depth > MAX_DEPTH {
            return Err(Self::error(span, "configuration nesting exceeds 64"));
        }
        match event {
            Event::Scalar(text, style, 0, None) => {
                if style != ScalarStyle::Plain {
                    return Ok(Value::String(text.into_owned()));
                }
                match text.as_ref() {
                    "" | "~" | "null" | "Null" | "NULL" => Ok(Value::Null),
                    "true" | "True" | "TRUE" => Ok(Value::Bool(true)),
                    "false" | "False" | "FALSE" => Ok(Value::Bool(false)),
                    _ => {
                        // Deliberately use JSON numeric syntax, preserving existing integer checks.
                        // Timestamps, yes/no, octal/hex-looking values remain strings.
                        if let Ok(value) = serde_json::from_str::<Value>(&text)
                            && value.is_number()
                        {
                            return Ok(value);
                        }
                        Ok(Value::String(text.into_owned()))
                    }
                }
            }
            Event::MappingStart(0, None) => {
                let mut map = Map::new();
                loop {
                    let (event, span) = self.next()?;
                    let key = match event {
                        Event::MappingEnd => break,
                        Event::Scalar(key, _, 0, None) if key != "<<" => key.into_owned(),
                        _ => {
                            return Err(Self::error(
                                span,
                                "mapping keys must be strings; tags, anchors and merge keys are unsupported",
                            ));
                        }
                    };
                    if map.contains_key(&key) {
                        return Err(Self::error(span, "duplicate mapping key"));
                    }
                    let (event, span) = self.next()?;
                    let value = self.node(event, span, depth + 1)?;
                    map.insert(key, value);
                }
                Ok(Value::Object(map))
            }
            Event::SequenceStart(0, None) => {
                let mut sequence = Vec::new();
                loop {
                    let (event, span) = self.next()?;
                    if event == Event::SequenceEnd {
                        break;
                    }
                    sequence.push(self.node(event, span, depth + 1)?);
                }
                Ok(Value::Array(sequence))
            }
            _ => Err(Self::error(
                span,
                "expected plain configuration data; tags, anchors and aliases are unsupported",
            )),
        }
    }
}
pub(super) fn parse_yaml(raw: &str) -> Result<Value, BoxError> {
    let mut reader = Reader {
        parser: Parser::new_from_str(raw),
        events: 0,
    };
    if reader.next()?.0 != Event::StreamStart {
        return Err("Missing YAML stream start".into());
    }
    if !matches!(reader.next()?.0, Event::DocumentStart(_)) {
        return Err("Expected one YAML document".into());
    }
    let (event, span) = reader.next()?;
    let value = reader.node(event, span, 0)?;
    if reader.next()?.0 != Event::DocumentEnd || reader.next()?.0 != Event::StreamEnd {
        return Err("Configuration must contain exactly one YAML document".into());
    }
    Ok(value)
}

// Config export uses only JSON's data model, so scalar quoting can use JSON escaping too.
// This emits YAML block collections; parsing and escaping remain in established libraries.
struct Output {
    bytes: Vec<u8>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("YAML configuration exceeds 32 MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn collection(value: &Value) -> bool {
    matches!(value,Value::Object(map) if !map.is_empty())
        || matches!(value,Value::Array(values) if !values.is_empty())
}
fn emit(value: &Value, depth: usize, output: &mut Output) -> Result<(), BoxError> {
    if depth > MAX_DEPTH {
        return Err("Configuration nesting exceeds 64".into());
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                write!(output, "{:indent$}", "", indent = depth * 2)?;
                if !key.is_empty()
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                    && !matches!(key.as_str(), "null" | "true" | "false")
                {
                    write!(output, "{key}")?;
                } else {
                    serde_json::to_writer(&mut *output, key)?;
                }
                output.write_all(b":")?;
                if collection(value) {
                    output.write_all(b"\n")?;
                    emit(value, depth + 1, output)?;
                } else {
                    output.write_all(b" ")?;
                    serde_json::to_writer(&mut *output, value)?;
                    output.write_all(b"\n")?;
                }
            }
        }
        Value::Array(values) if !values.is_empty() => {
            for value in values {
                write!(output, "{:indent$}-", "", indent = depth * 2)?;
                if collection(value) {
                    output.write_all(b"\n")?;
                    emit(value, depth + 1, output)?;
                } else {
                    output.write_all(b" ")?;
                    serde_json::to_writer(&mut *output, value)?;
                    output.write_all(b"\n")?;
                }
            }
        }
        _ => {
            serde_json::to_writer(&mut *output, value)?;
            output.write_all(b"\n")?;
        }
    }
    Ok(())
}
pub(super) fn to_yaml(value: &Value) -> Result<String, BoxError> {
    let mut output = Output { bytes: Vec::new() };
    emit(value, 0, &mut output)?;
    String::from_utf8(output.bytes).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::Path;
    #[test]
    fn yaml_and_legacy_json_scenarios_have_identical_typed_configuration() {
        for (yaml, json) in [
            (
                include_str!("../../../config/scenarios/local.yaml"),
                include_str!("../../../config/scenarios/local.json"),
            ),
            (
                include_str!("../../../config/scenarios/unix.yaml"),
                include_str!("../../../config/scenarios/unix.json"),
            ),
            (
                include_str!("../../../config/scenarios/central.yaml"),
                include_str!("../../../config/scenarios/central.json"),
            ),
            (
                include_str!("../../../config/scenarios/edge.yaml"),
                include_str!("../../../config/scenarios/edge.json"),
            ),
        ] {
            let yaml = Config::parse(Path::new("scenario.yaml"), yaml).unwrap();
            let json = Config::parse(Path::new("scenario.json"), json).unwrap();
            assert_eq!(
                serde_json::to_value(&yaml).unwrap(),
                serde_json::to_value(json).unwrap()
            );
            let emitted = yaml.to_yaml().unwrap();
            let roundtrip = Config::parse(Path::new("roundtrip.YML"), &emitted).unwrap();
            assert_eq!(
                serde_json::to_value(yaml).unwrap(),
                serde_json::to_value(roundtrip).unwrap()
            );
        }
    }
    #[test]
    fn rejects_duplicate_keys_at_all_depths_including_escaped_keys() {
        for text in [
            "a: 1\na: 2",
            "a: {b: 1, b: 2}",
            "a: [{b: 1, b: 2}]",
            "a: 1\n\"\\u0061\": 2",
        ] {
            assert!(
                parse_yaml(text)
                    .unwrap_err()
                    .to_string()
                    .contains("duplicate mapping key")
            );
        }
    }
    #[test]
    fn rejects_tags_anchors_aliases_merge_keys_complex_keys_and_multiple_documents() {
        for text in [
            "a: !!str 1",
            "a: !custom value",
            "a: &anchor hello\nb: *anchor",
            "a: &anchor [1, 2]",
            "a: &anchor {b: 1}",
            "a: *missing",
            "<<: {a: 1}",
            "? [a, b]\n: value",
            "a: 1\n---\na: 2",
            "",
            "a: [1, 2",
        ] {
            assert!(parse_yaml(text).is_err(), "{text}");
        }
    }
    #[test]
    fn quoted_strings_and_block_scalars_preserve_types_and_escaping() {
        let value=parse_yaml("# comment\nnumber: 12\ntruth: true\nempty:\ntext: 'true'\ndate: 2030-01-01T00:00:00Z\nlegacy: yes\ncode: '0012'\nmultiline: |\n  first\n  second\n").unwrap();
        assert_eq!(value["number"], 12);
        assert_eq!(value["truth"], true);
        assert_eq!(value["empty"], Value::Null);
        assert_eq!(value["text"], "true");
        assert_eq!(value["legacy"], "yes");
        assert_eq!(value["code"], "0012");
        assert_eq!(value["date"], "2030-01-01T00:00:00Z");
        assert_eq!(value["multiline"], "first\nsecond\n");
        let value = serde_json::json!({"true":"false","<<not_merge":"# : [ ] { }","unicode":"Grüße 🦀","escaped":"\u{0}\t\r\n\\\"","empty_array":[],"empty_object":{},"values":[false,null,18446744073709551615u64,-12,"null","12",[1,2]]});
        assert_eq!(parse_yaml(&to_yaml(&value).unwrap()).unwrap(), value);
    }
    #[test]
    fn yaml_and_json_reject_unknown_fields_and_wrong_types_without_fallback() {
        let json: Value =
            serde_json::from_str(include_str!("../../../config/scenarios/unix.json")).unwrap();
        let mut invalid = json;
        invalid["service"]["typo"] = Value::Bool(true);
        for file in ["test.yaml", "test.json"] {
            assert!(Config::parse(Path::new(file), &invalid.to_string()).is_err());
        }
        let baseline = include_str!("../../../config/scenarios/unix.yaml");
        for replacement in [
            "schema_version: '1'",
            "schema_version: true",
            "schema_version: 0x1",
        ] {
            assert!(
                Config::parse(
                    Path::new("test.yaml"),
                    &baseline.replace("schema_version: 1", replacement)
                )
                .is_err()
            );
        }
        assert!(Config::parse(Path::new("test.json"), baseline).is_err());
        assert!(Config::parse(Path::new("unknown.cfg"), baseline).is_err());
    }
    #[test]
    fn nesting_and_input_output_budgets_are_enforced() {
        assert!(parse_yaml(&format!("{}0{}", "[".repeat(66), "]".repeat(66))).is_err());
        let mut reader = Reader {
            parser: Parser::new_from_str("a: 1"),
            events: MAX_EVENTS,
        };
        assert!(reader.next().is_err());
        let mut output = Output {
            bytes: vec![0; MAX_BYTES],
        };
        assert!(output.write_all(b"x").is_err());
        let oversized = " ".repeat(MAX_BYTES + 1);
        assert!(Config::parse(Path::new("test.yaml"), &oversized).is_err());
        assert!(Config::parse(Path::new("test.json"), &oversized).is_err());
    }
}

#[cfg(test)]
mod functional {
    use super::*;
    use crate::{
        config::Config,
        test_support::{self, Rng},
    };
    #[test]
    fn functional_seeded_yaml_json_mutations_either_reject_or_roundtrip() {
        let corpus = [
            include_bytes!("../../../config/scenarios/local.yaml").as_slice(),
            include_bytes!("../../../config/scenarios/local.json").as_slice(),
            b"a: {b: [true, null, 1, \"\\u0000\"]}".as_slice(),
            b"a: &x [*x]".as_slice(),
            b"a: 1\na: 2".as_slice(),
        ];
        let cases = test_support::fuzz_cases();
        let mut rng = Rng(test_support::seed());
        let mut accepted = 0;
        for case in 0..cases {
            let base = corpus[rng.pick(corpus.len())];
            let input = if case % 32 == 0 {
                base.to_vec()
            } else {
                rng.mutate(base, 8192)
            };
            let Ok(text) = std::str::from_utf8(&input) else {
                continue;
            };
            let result = std::panic::catch_unwind(|| parse_yaml(text));
            let value = result.unwrap_or_else(|_| {
                panic!(
                    "YAML panic at seed=0x{:016x}, case={case}, input={text:?}",
                    test_support::seed()
                )
            });
            if let Ok(value) = value {
                accepted += 1;
                let emitted = to_yaml(&value).unwrap();
                assert_eq!(parse_yaml(&emitted).unwrap(), value);
            }
            for path in ["mutated.yaml", "mutated.json"] {
                if let Ok(config) = Config::parse(std::path::Path::new(path), text) {
                    config.validate().unwrap();
                    let emitted = config.to_yaml().unwrap();
                    let loaded =
                        Config::parse(std::path::Path::new("roundtrip.yaml"), &emitted).unwrap();
                    assert_eq!(
                        serde_json::to_value(loaded).unwrap(),
                        serde_json::to_value(config).unwrap()
                    );
                }
            }
        }
        assert!(accepted > 0);
        eprintln!("Configuration mutations: {cases} cases, {accepted} YAML values roundtripped");
    }
}
