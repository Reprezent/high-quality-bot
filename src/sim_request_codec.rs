use anyhow::{Context, Result, anyhow};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use serde_json::Value;
use std::sync::OnceLock;

use crate::mop_proto::mop::RaidSimRequest;

pub const INPUT_FORMAT_GEAR_JSON: &str = "gear-json";
pub const MOP_UPSTREAM_REVISION: &str = env!("MOP_UPSTREAM_REVISION");

fn proto_descriptor_pool() -> Result<&'static DescriptorPool> {
    static DESCRIPTOR_POOL: OnceLock<Result<DescriptorPool, String>> = OnceLock::new();

    let pool = DESCRIPTOR_POOL.get_or_init(|| {
        DescriptorPool::decode(crate::mop_proto::mop::DESCRIPTOR_SET_BYTES)
            .map_err(|error| format!("failed to decode protobuf descriptor set: {error}"))
    });

    match pool {
        Ok(pool) => Ok(pool),
        Err(error) => Err(anyhow!(error.clone())),
    }
}

fn message_descriptor(message_name: &str) -> Result<MessageDescriptor> {
    proto_descriptor_pool()?
        .get_message_by_name(message_name)
        .ok_or_else(|| anyhow!("{message_name} descriptor not found in descriptor set"))
}

pub fn parse_protojson_message<T>(message_name: &str, value: &Value) -> Result<T>
where
    T: Message + Default,
{
    let descriptor = message_descriptor(message_name)?;
    let payload = serde_json::to_string(value)
        .with_context(|| format!("failed to serialize {message_name} payload as JSON"))?;
    let mut deserializer = serde_json::Deserializer::from_str(&payload);
    let dynamic = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .with_context(|| format!("failed to decode {message_name} ProtoJSON"))?;

    dynamic
        .transcode_to::<T>()
        .with_context(|| format!("failed to transcode dynamic {message_name} message"))
}

pub fn parse_raid_sim_request(payload: &Value) -> Result<RaidSimRequest> {
    parse_protojson_message("proto.RaidSimRequest", payload)
}

pub fn protojson_message_to_value<T>(message_name: &str, message: &T) -> Result<Value>
where
    T: Message,
{
    let descriptor = message_descriptor(message_name)?;
    let bytes = message.encode_to_vec();
    let dynamic = DynamicMessage::decode(descriptor, &mut bytes.as_slice())
        .with_context(|| format!("failed to decode {message_name} protobuf bytes"))?;

    serde_json::to_value(dynamic)
        .with_context(|| format!("failed to serialize {message_name} as ProtoJSON"))
}
