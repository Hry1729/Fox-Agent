//! Canonical JSON for every place where the exact bytes of a JSON document
//! matter: hashes, frozen snapshots, strings compared with SQL literals or
//! trigger checks, idempotency keys and audit keys.
//!
//! # Why this exists
//!
//! `serde_json::Map` is a `BTreeMap` (keys sorted) unless the `preserve_order`
//! feature is enabled, in which case it is an `IndexMap` (insertion order).
//! Cargo unifies features across the whole binary, so the moment any
//! dependency turns `preserve_order` on (since `afb0764`: `agent-client-protocol`
//! via `fox-agent-backend/acp`), every `serde_json::to_string(&value)` and every
//! `value.to_string()` in the desktop Host silently switches from sorted keys to
//! insertion order. Hashes, the `run_execution_profiles` trigger literals
//! (`'{"id":"durable_v2","schemaVersion":1}'`) and the `json_each` key-order
//! checks all drifted.
//!
//! The functions here reproduce the original (`BTreeMap`) output byte for byte
//! no matter which feature set the binary was built with:
//!
//! * every JSON **object that comes from a map** (`serde_json::Map`/`Value`,
//!   `HashMap`, `BTreeMap`, `IndexMap`, ...) is written with its keys sorted by
//!   byte order (the `String` `Ord`, the same order `BTreeMap` used);
//! * a Rust **struct** keeps its field declaration order, exactly as
//!   `serde_json` always wrote it (structs were never affected by the feature);
//! * scalars, escaping, compact / pretty layout are delegated to `serde_json`,
//!   so the bytes are serde_json's own.
//!
//! One deliberate difference from plain serde_json: anything else that
//! serializes as a map (`#[serde(flatten)]` structs, hand-written impls that
//! call `serialize_map`) is sorted too, because serde gives no way to tell it
//! apart from a `HashMap`. No hashed / compared type in the Host uses those
//! (audited 2026-10-21: the only `flatten` is a Tauri command response).
//!
//! Rule for the Kernel and the desktop Host (which re-exports this module as
//! `crate::canonical_json`): whenever JSON bytes are hashed, stored to be
//! compared later, or compared with a literal, serialize them with this
//! module, never with `serde_json::to_string` / `to_vec` / `Value::to_string`
//! directly. [`CanonicalJson::canonical_string`] is the drop-in replacement
//! for `value.to_string()`.
//!
//! It lives in `fox-agent-kernel` (the lowest crate of the desktop binary that
//! serializes JSON) so the Kernel's own durable payloads use it too. This
//! crate's tests build with `preserve_order` on (dev-dependency), the same
//! condition as the desktop binary, so the guarantees below are exercised by
//! `cargo test -p fox-agent-kernel` on any host.
use serde::ser::{self, Serialize, Serializer};
use serde_json::Value;

/// `value.canonical_string()`: canonical replacement for `value.to_string()`.
pub trait CanonicalJson {
    /// Compact canonical JSON (sorted object keys).
    fn canonical_string(&self) -> String;
}

impl CanonicalJson for Value {
    fn canonical_string(&self) -> String {
        value_string(self)
    }
}

/// Compact canonical JSON of any serializable value (see the module docs).
pub fn to_string<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(&Canonical::of(value)?)
}

/// Compact canonical JSON bytes.
pub fn to_vec<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&Canonical::of(value)?)
}

/// Pretty canonical JSON (serde_json's pretty layout, sorted map keys).
pub fn to_string_pretty<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(&Canonical::of(value)?)
}

/// Pretty canonical JSON bytes.
pub fn to_vec_pretty<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec_pretty(&Canonical::of(value)?)
}

/// Canonical JSON written to a writer.
pub fn to_writer<W: std::io::Write, T: ?Sized + Serialize>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    serde_json::to_writer(writer, &Canonical::of(value)?)
}

/// Pretty canonical JSON written to a writer.
pub fn to_writer_pretty<W: std::io::Write, T: ?Sized + Serialize>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    serde_json::to_writer_pretty(writer, &Canonical::of(value)?)
}

/// Canonical replacement for `value.to_string()` on a [`Value`]. Infallible,
/// like `Display` (a `Value` always serializes).
pub fn value_string(value: &Value) -> String {
    serde_json::to_string(&SortedValue(value)).expect("a serde_json::Value always serializes")
}

/// Canonical bytes of a [`Value`].
pub fn value_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&SortedValue(value)).expect("a serde_json::Value always serializes")
}

/// A copy of `value` whose objects were rebuilt with their keys inserted in
/// sorted order. Under `preserve_order` a later plain serialization of the copy
/// is canonical too; use it when a `Value` has to be embedded in a struct or
/// handed to code that serializes it itself.
pub fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                out.insert(key.clone(), sorted(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Serialize a `Value` with every object's keys sorted, without copying it.
struct SortedValue<'a>(&'a Value);

impl Serialize for SortedValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use ser::{SerializeMap, SerializeSeq};
        match self.0 {
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                let mut out = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, &SortedValue(value))?;
                }
                out.end()
            }
            Value::Array(items) => {
                let mut out = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(&SortedValue(item))?;
                }
                out.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// serde_json's private marker for `RawValue` (feature `raw_value`).
const RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";

/// An intermediate tree that remembers whether an object came from a struct
/// (keep field order) or from a map (sort keys).
enum Node {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    I128(i128),
    U128(u128),
    F32(f32),
    F64(f64),
    Str(String),
    Seq(Vec<Node>),
    /// Struct fields / enum wrappers: declaration order, as serde_json wrote it.
    Ordered(Vec<(String, Node)>),
    /// Map entries: written sorted by key.
    Map(Vec<(String, Node)>),
}

struct Canonical(Node);

impl Canonical {
    fn of<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Self> {
        value.serialize(NodeSerializer).map(Canonical)
    }
}

impl Serialize for Canonical {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use ser::{SerializeMap, SerializeSeq};
        match self {
            Node::Null => serializer.serialize_unit(),
            Node::Bool(value) => serializer.serialize_bool(*value),
            Node::I64(value) => serializer.serialize_i64(*value),
            Node::U64(value) => serializer.serialize_u64(*value),
            Node::I128(value) => serializer.serialize_i128(*value),
            Node::U128(value) => serializer.serialize_u128(*value),
            Node::F32(value) => serializer.serialize_f32(*value),
            Node::F64(value) => serializer.serialize_f64(*value),
            Node::Str(value) => serializer.serialize_str(value),
            Node::Seq(items) => {
                let mut out = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(item)?;
                }
                out.end()
            }
            Node::Ordered(entries) => {
                let mut out = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, value)?;
                }
                out.end()
            }
            Node::Map(entries) => {
                let mut sorted: Vec<&(String, Node)> = entries.iter().collect();
                // Stable: a duplicate key (only possible from a hand-written
                // Serialize impl) keeps its relative order, as serde_json would.
                sorted.sort_by(|left, right| left.0.cmp(&right.0));
                let mut out = serializer.serialize_map(Some(sorted.len()))?;
                for (key, value) in sorted {
                    out.serialize_entry(key, value)?;
                }
                out.end()
            }
        }
    }
}

fn value_node(value: &Value) -> Node {
    match value {
        Value::Null => Node::Null,
        Value::Bool(value) => Node::Bool(*value),
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                Node::U64(value)
            } else if let Some(value) = number.as_i64() {
                Node::I64(value)
            } else {
                Node::F64(number.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(value) => Node::Str(value.clone()),
        Value::Array(items) => Node::Seq(items.iter().map(value_node).collect()),
        Value::Object(map) => Node::Map(
            map.iter()
                .map(|(key, value)| (key.clone(), value_node(value)))
                .collect(),
        ),
    }
}

/// Turn a map key into the string serde_json would have written for it.
fn key_string(node: Node) -> serde_json::Result<String> {
    match node {
        Node::Str(value) => Ok(value),
        Node::Bool(value) => Ok(value.to_string()),
        Node::I64(value) => Ok(value.to_string()),
        Node::U64(value) => Ok(value.to_string()),
        Node::I128(value) => Ok(value.to_string()),
        Node::U128(value) => Ok(value.to_string()),
        Node::F32(value) if value.is_finite() => Ok(serde_json::to_string(&value)?),
        Node::F64(value) if value.is_finite() => Ok(serde_json::to_string(&value)?),
        _ => Err(ser::Error::custom("key must be a string")),
    }
}

struct NodeSerializer;

struct SeqNode(Vec<Node>);
struct VariantSeqNode(&'static str, Vec<Node>);
struct MapNode {
    entries: Vec<(String, Node)>,
    pending_key: Option<String>,
}
struct StructNode {
    entries: Vec<(String, Node)>,
    raw: Option<String>,
    is_raw: bool,
}
struct VariantStructNode(&'static str, Vec<(String, Node)>);

impl Serializer for NodeSerializer {
    type Ok = Node;
    type Error = serde_json::Error;
    type SerializeSeq = SeqNode;
    type SerializeTuple = SeqNode;
    type SerializeTupleStruct = SeqNode;
    type SerializeTupleVariant = VariantSeqNode;
    type SerializeMap = MapNode;
    type SerializeStruct = StructNode;
    type SerializeStructVariant = VariantStructNode;

    fn serialize_bool(self, value: bool) -> serde_json::Result<Node> {
        Ok(Node::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> serde_json::Result<Node> {
        Ok(Node::I64(value.into()))
    }
    fn serialize_i16(self, value: i16) -> serde_json::Result<Node> {
        Ok(Node::I64(value.into()))
    }
    fn serialize_i32(self, value: i32) -> serde_json::Result<Node> {
        Ok(Node::I64(value.into()))
    }
    fn serialize_i64(self, value: i64) -> serde_json::Result<Node> {
        Ok(Node::I64(value))
    }
    fn serialize_i128(self, value: i128) -> serde_json::Result<Node> {
        Ok(Node::I128(value))
    }
    fn serialize_u8(self, value: u8) -> serde_json::Result<Node> {
        Ok(Node::U64(value.into()))
    }
    fn serialize_u16(self, value: u16) -> serde_json::Result<Node> {
        Ok(Node::U64(value.into()))
    }
    fn serialize_u32(self, value: u32) -> serde_json::Result<Node> {
        Ok(Node::U64(value.into()))
    }
    fn serialize_u64(self, value: u64) -> serde_json::Result<Node> {
        Ok(Node::U64(value))
    }
    fn serialize_u128(self, value: u128) -> serde_json::Result<Node> {
        Ok(Node::U128(value))
    }
    fn serialize_f32(self, value: f32) -> serde_json::Result<Node> {
        Ok(Node::F32(value))
    }
    fn serialize_f64(self, value: f64) -> serde_json::Result<Node> {
        Ok(Node::F64(value))
    }
    fn serialize_char(self, value: char) -> serde_json::Result<Node> {
        Ok(Node::Str(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> serde_json::Result<Node> {
        Ok(Node::Str(value.to_owned()))
    }
    fn serialize_bytes(self, value: &[u8]) -> serde_json::Result<Node> {
        Ok(Node::Seq(value.iter().map(|byte| Node::U64((*byte).into())).collect()))
    }
    fn serialize_none(self) -> serde_json::Result<Node> {
        Ok(Node::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> serde_json::Result<Node> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> serde_json::Result<Node> {
        Ok(Node::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<Node> {
        Ok(Node::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> serde_json::Result<Node> {
        Ok(Node::Str(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> serde_json::Result<Node> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> serde_json::Result<Node> {
        Ok(Node::Ordered(vec![(variant.to_owned(), value.serialize(NodeSerializer)?)]))
    }
    fn serialize_seq(self, len: Option<usize>) -> serde_json::Result<SeqNode> {
        Ok(SeqNode(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> serde_json::Result<SeqNode> {
        Ok(SeqNode(Vec::with_capacity(len)))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> serde_json::Result<SeqNode> {
        Ok(SeqNode(Vec::with_capacity(len)))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> serde_json::Result<VariantSeqNode> {
        Ok(VariantSeqNode(variant, Vec::with_capacity(len)))
    }
    fn serialize_map(self, len: Option<usize>) -> serde_json::Result<MapNode> {
        Ok(MapNode {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            pending_key: None,
        })
    }
    fn serialize_struct(self, name: &'static str, len: usize) -> serde_json::Result<StructNode> {
        Ok(StructNode {
            entries: Vec::with_capacity(len),
            raw: None,
            is_raw: name == RAW_VALUE_TOKEN,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> serde_json::Result<VariantStructNode> {
        Ok(VariantStructNode(variant, Vec::with_capacity(len)))
    }
}

impl ser::SerializeSeq for SeqNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.0.push(value.serialize(NodeSerializer)?);
        Ok(())
    }
    fn end(self) -> serde_json::Result<Node> {
        Ok(Node::Seq(self.0))
    }
}

impl ser::SerializeTuple for SeqNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> serde_json::Result<Node> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SeqNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> serde_json::Result<Node> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleVariant for VariantSeqNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.1.push(value.serialize(NodeSerializer)?);
        Ok(())
    }
    fn end(self) -> serde_json::Result<Node> {
        Ok(Node::Ordered(vec![(self.0.to_owned(), Node::Seq(self.1))]))
    }
}

impl ser::SerializeMap for MapNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> serde_json::Result<()> {
        self.pending_key = Some(key_string(key.serialize(NodeSerializer)?)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| <serde_json::Error as ser::Error>::custom("map value without a key"))?;
        self.entries.push((key, value.serialize(NodeSerializer)?));
        Ok(())
    }
    fn end(self) -> serde_json::Result<Node> {
        Ok(Node::Map(self.entries))
    }
}

impl ser::SerializeStruct for StructNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        let node = value.serialize(NodeSerializer)?;
        if self.is_raw && key == RAW_VALUE_TOKEN {
            if let Node::Str(raw) = node {
                self.raw = Some(raw);
                return Ok(());
            }
        }
        self.entries.push((key.to_owned(), node));
        Ok(())
    }
    fn end(self) -> serde_json::Result<Node> {
        if let Some(raw) = self.raw {
            // A RawValue is opaque text; canonicalize what it contains.
            let value: Value = serde_json::from_str(&raw)?;
            return Ok(value_node(&value));
        }
        Ok(Node::Ordered(self.entries))
    }
}

impl ser::SerializeStructVariant for VariantStructNode {
    type Ok = Node;
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        self.1.push((key.to_owned(), value.serialize(NodeSerializer)?));
        Ok(())
    }
    fn end(self) -> serde_json::Result<Node> {
        Ok(Node::Ordered(vec![(self.0.to_owned(), Node::Ordered(self.1))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};

    /// The reference: what serde_json wrote before `preserve_order` was
    /// unified into the binary — every object key sorted (BTreeMap order).
    fn btree_reference(value: &Value) -> String {
        fn walk(value: &Value, out: &mut String) {
            match value {
                Value::Object(map) => {
                    let sorted: BTreeMap<&String, &Value> = map.iter().collect();
                    out.push('{');
                    for (index, (key, value)) in sorted.into_iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push_str(&serde_json::to_string(key).unwrap());
                        out.push(':');
                        walk(value, out);
                    }
                    out.push('}');
                }
                Value::Array(items) => {
                    out.push('[');
                    for (index, item) in items.iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        walk(item, out);
                    }
                    out.push(']');
                }
                scalar => out.push_str(&serde_json::to_string(scalar).unwrap()),
            }
        }
        let mut out = String::new();
        walk(value, &mut out);
        out
    }

    /// Whether this build actually has `preserve_order` (the desktop binary
    /// does while `fox-agent-backend/acp` pulls in agent-client-protocol;
    /// this crate's tests do through the dev-dependency).
    pub(crate) fn preserve_order_is_active() -> bool {
        serde_json::to_string(&json!({"b": 1, "a": 2})).unwrap() == r#"{"b":1,"a":2}"#
    }

    #[test]
    fn tests_run_under_preserve_order_like_the_desktop_binary() {
        // Without this the tests below would pass trivially (BTreeMap).
        assert!(
            preserve_order_is_active(),
            "fox-agent-kernel tests must build serde_json with preserve_order (dev-dependency)"
        );
        // And the plain serializer really does drift under it.
        assert_ne!(
            serde_json::to_string(&json!({"schemaVersion": 1, "id": "durable_v2"})).unwrap(),
            r#"{"id":"durable_v2","schemaVersion":1}"#
        );
    }

    #[test]
    fn canonical_output_sorts_keys_even_under_preserve_order() {
        let value = json!({
            "zeta": 1,
            "alpha": {"y": [ {"d": null, "c": true} ], "x": "é\"\n"},
            "Mid": 2.5,
            "_": -3,
            "10": "ten",
            "9": "nine",
        });
        let expected = r#"{"10":"ten","9":"nine","Mid":2.5,"_":-3,"alpha":{"x":"é\"\n","y":[{"c":true,"d":null}]},"zeta":1}"#;
        assert_eq!(value_string(&value), expected);
        assert_eq!(to_string(&value).unwrap(), expected);
        assert_eq!(String::from_utf8(value_bytes(&value)).unwrap(), expected);
        assert_eq!(String::from_utf8(to_vec(&value).unwrap()).unwrap(), expected);
        assert_eq!(btree_reference(&value), expected);
        assert_eq!(serde_json::to_string(&sorted(&value)).unwrap(), expected);
        assert_eq!(
            to_string_pretty(&value).unwrap(),
            serde_json::to_string_pretty(&sorted(&value)).unwrap()
        );
    }

    #[test]
    fn insertion_order_never_changes_canonical_bytes() {
        let left = json!({"schemaVersion": 1, "id": "durable_v2"});
        let right = json!({"id": "durable_v2", "schemaVersion": 1});
        assert_eq!(value_string(&left), value_string(&right));
        assert_eq!(value_string(&left), r#"{"id":"durable_v2","schemaVersion":1}"#);
        assert_eq!(left.canonical_string(), r#"{"id":"durable_v2","schemaVersion":1}"#);
        let mut map = serde_json::Map::new();
        map.insert("b".into(), json!(1));
        map.insert("a".into(), json!(2));
        assert_eq!(to_string(&map).unwrap(), r#"{"a":2,"b":1}"#);
        let hash: HashMap<&str, i32> = [("q", 1), ("p", 2), ("r", 3)].into_iter().collect();
        assert_eq!(to_string(&hash).unwrap(), r#"{"p":2,"q":1,"r":3}"#);
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Policy {
        schema_version: u32,
        id: &'static str,
        nested: Value,
        tagged: Tagged,
        maybe: Option<u8>,
        small: f32,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Tagged {
        Wrapped { zed: u8, apple: u8 },
    }

    #[test]
    fn structs_keep_declaration_order_and_their_maps_are_sorted() {
        let policy = Policy {
            schema_version: 1,
            id: "standard_v1",
            nested: json!({"z": 1, "a": 2}),
            tagged: Tagged::Wrapped { zed: 1, apple: 2 },
            maybe: None,
            small: 0.1,
        };
        // Byte-identical to what serde_json produced with BTreeMap maps:
        // struct fields in declaration order, map keys sorted.
        assert_eq!(
            to_string(&policy).unwrap(),
            r#"{"schemaVersion":1,"id":"standard_v1","nested":{"a":2,"z":1},"tagged":{"wrapped":{"zed":1,"apple":2}},"maybe":null,"small":0.1}"#
        );
        // A struct without maps is exactly serde_json's own output.
        #[derive(Serialize)]
        struct Plain<'a> {
            b: u8,
            a: &'a str,
            list: Vec<i64>,
        }
        let plain = Plain { b: 1, a: "x", list: vec![-1, 2] };
        assert_eq!(to_string(&plain).unwrap(), serde_json::to_string(&plain).unwrap());
        assert_eq!(
            to_string_pretty(&plain).unwrap(),
            serde_json::to_string_pretty(&plain).unwrap()
        );
    }

    #[test]
    fn numbers_and_strings_are_serde_json_bytes() {
        let value = json!([u64::MAX, i64::MIN, 1.5e300, 0.1, "\u{1f}\\", "中文"]);
        assert_eq!(value_string(&value), serde_json::to_string(&value).unwrap());
    }
}
