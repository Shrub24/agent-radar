use std::error::Error;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

use agent_radar::control_plane::public_contracts::observation::{
    ObservationGetParams, ObservationGetResponse, ObservationSnapshot,
};
use agent_radar::control_plane::public_contracts::{
    Handoff, HandoffInbox, HandoffPublishParams, HandoffReceipt, LaunchIdentityMarker,
    ManagedLaunchMetadata, PingResult,
};
use agent_radar::control_plane::registry::publication::FactsSnapshot;
use agent_radar::control_plane::registry::spawn::PublicSpawnExitObservation;

fn schema<T: JsonSchema>(title: &str, serialize: bool) -> Result<Value, Box<dyn Error>> {
    let settings = if serialize {
        SchemaSettings::draft2020_12().for_serialize()
    } else {
        SchemaSettings::draft2020_12().for_deserialize()
    };
    let generator = settings.into_generator();
    let mut value = serde_json::to_value(generator.into_root_schema_for::<T>())?;
    value["title"] = json!(title);
    Ok(value)
}

fn remove_null(schema: &mut Value) {
    if let Some(types) = schema.get_mut("type").and_then(Value::as_array_mut) {
        types.retain(|value| value != "null");
        if types.len() == 1 {
            schema["type"] = types[0].clone();
        }
    }
    if let Some(variants) = schema.get_mut("anyOf").and_then(Value::as_array_mut) {
        variants.retain(|variant| variant.get("type") != Some(&json!("null")));
        if variants.len() == 1 {
            let only = variants[0].clone();
            *schema = only;
        }
    }
}

fn close_serialized_objects(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if object.contains_key("properties") {
                object.insert("additionalProperties".into(), Value::Bool(false));
            }
            for value in object.values_mut() {
                close_serialized_objects(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                close_serialized_objects(item);
            }
        }
        _ => {}
    }
}

fn normalize_serialization_schema(schema: &mut Value, name: &str) {
    close_serialized_objects(schema);
    if name != "facts-snapshot" {
        return;
    }
    let properties = schema.get_mut("properties").and_then(Value::as_object_mut);
    if let Some(properties) = properties {
        for name in [
            "model",
            "provider",
            "thinking",
            "context_window",
            "session_name",
            "background_started_unix_ms",
        ] {
            if let Some(property) = properties.get_mut(name) {
                remove_null(property);
            }
        }
    }
}

fn has_maximum(schema: &Value, property: &str, expected: u64) -> bool {
    match schema {
        Value::Object(object) => {
            let property_matches = object
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get(property))
                .and_then(|value| value.get("maximum"))
                .and_then(Value::as_u64)
                == Some(expected);
            property_matches
                || object
                    .values()
                    .any(|value| has_maximum(value, property, expected))
        }
        Value::Array(items) => items
            .iter()
            .any(|value| has_maximum(value, property, expected)),
        _ => false,
    }
}

fn verify_safe_integer_bounds(schema: &Value) -> Result<(), Box<dyn Error>> {
    let expected = agent_radar::model::MAX_SAFE_JSON_INTEGER;
    for property in ["tokens", "window", "background_started_unix_ms"] {
        if !has_maximum(schema, property, expected) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("generated facts schema is missing {property} maximum {expected}"),
            )
            .into());
        }
    }
    Ok(())
}

fn write_pair<T: JsonSchema>(directory: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    for (direction, serialize) in [("serialize", true), ("deserialize", false)] {
        let title = format!("{name}-{direction}");
        let mut value = schema::<T>(&title, serialize)?;
        if name == "facts-snapshot" {
            verify_safe_integer_bounds(&value)?;
        }
        if serialize {
            normalize_serialization_schema(&mut value, name);
        }
        let path = directory.join(format!("{name}.{direction}.schema.json"));
        let mut contents = serde_json::to_string_pretty(&value)?;
        contents.push('\n');
        fs::write(path, contents)?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut output = PathBuf::from("pi-radar/schema/public/v1");
    while let Some(arg) = args.next() {
        if arg == "--output" {
            output = args
                .next()
                .map(PathBuf::from)
                .ok_or("--output requires a directory")?;
        } else {
            return Err(format!("unknown argument: {}", arg.to_string_lossy()).into());
        }
    }
    fs::create_dir_all(&output)?;
    write_pair::<PingResult>(&output, "daemon-ping")?;
    write_pair::<HandoffPublishParams>(&output, "handoff-publish-params")?;
    write_pair::<HandoffInbox>(&output, "handoff-inbox")?;
    write_pair::<Handoff>(&output, "handoff")?;
    write_pair::<HandoffReceipt>(&output, "handoff-receipt")?;
    write_pair::<ManagedLaunchMetadata>(&output, "managed-launch-metadata")?;
    write_pair::<LaunchIdentityMarker>(&output, "launch-identity-marker")?;
    write_pair::<FactsSnapshot>(&output, "facts-snapshot")?;
    write_pair::<PublicSpawnExitObservation>(&output, "spawn-exit-observation")?;
    write_pair::<ObservationGetParams>(&output, "observation-get-params")?;
    write_pair::<ObservationSnapshot>(&output, "observation-snapshot")?;
    write_pair::<ObservationGetResponse>(&output, "observation-get-response")?;
    Ok(())
}
