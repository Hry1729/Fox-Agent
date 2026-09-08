fn main() {
    println!("{}", serde_json::to_string_pretty(&fox_engine_protocol::schema_bundle()).expect("serializable protocol schema"));
}
