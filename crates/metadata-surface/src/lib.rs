//! Pallet-level surface of SCALE-encoded V16 runtime metadata.
//!
//! A re-anchor replaces a binary metadata fixture. This crate renders each
//! anchor as stable, line-oriented text so the semantic change can be reviewed:
//! every pallet's calls, events and errors with their indices and resolved
//! field types, constants with values, storage layouts and defaults, runtime
//! APIs, the extrinsic format and the outer enums. Docs are omitted on purpose:
//! they change with every SDK bump and say nothing about encoding.
use codec::Decode;
use frame_metadata::{
    v16::{RuntimeMetadataV16, StorageEntryType},
    RuntimeMetadata, RuntimeMetadataPrefixed, META_RESERVED,
};
use scale_info::{form::PortableForm, Field, PortableRegistry, TypeDef};
use std::collections::BTreeMap;

/// Nesting limit for anonymous types and named type parameters.
const MAX_DEPTH: usize = 8;

/// Decode a full `RuntimeMetadataPrefixed` blob, rejecting trailing bytes and
/// anything other than V16.
pub fn decode(bytes: &[u8]) -> Result<RuntimeMetadataV16, String> {
    let mut input = bytes;
    let prefixed = RuntimeMetadataPrefixed::decode(&mut input)
        .map_err(|error| format!("metadata does not SCALE-decode: {error}"))?;
    if !input.is_empty() {
        return Err(format!("metadata has {} trailing bytes", input.len()));
    }
    if prefixed.0 != META_RESERVED {
        return Err("metadata magic prefix mismatch".into());
    }
    match prefixed.1 {
        RuntimeMetadata::V16(metadata) => Ok(metadata),
        _ => Err("expected Metadata V16".into()),
    }
}

fn name(registry: &PortableRegistry, id: u32, depth: usize) -> String {
    let Some(ty) = registry.resolve(id) else {
        return format!("<missing type {id}>");
    };
    if depth > MAX_DEPTH {
        return format!("#{id}");
    }
    if !ty.path.segments.is_empty() {
        let mut rendered = ty.path.segments.join("::");
        let params: Vec<_> = ty
            .type_params
            .iter()
            .map(|param| match param.ty {
                Some(param) => name(registry, param.id, depth + 1),
                None => "_".into(),
            })
            .collect();
        if !params.is_empty() {
            rendered += &format!("<{}>", params.join(", "));
        }
        return rendered;
    }
    match &ty.type_def {
        TypeDef::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
        TypeDef::Sequence(seq) => format!("Vec<{}>", name(registry, seq.type_param.id, depth + 1)),
        TypeDef::Array(array) => format!(
            "[{}; {}]",
            name(registry, array.type_param.id, depth + 1),
            array.len
        ),
        TypeDef::Tuple(tuple) => format!(
            "({})",
            tuple
                .fields
                .iter()
                .map(|field| name(registry, field.id, depth + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDef::Compact(compact) => format!(
            "Compact<{}>",
            name(registry, compact.type_param.id, depth + 1)
        ),
        TypeDef::BitSequence(bits) => format!(
            "BitVec<{}, {}>",
            name(registry, bits.bit_store_type.id, depth + 1),
            name(registry, bits.bit_order_type.id, depth + 1)
        ),
        other => format!("anon {}", definition(registry, other, depth + 1)),
    }
}

fn fields(registry: &PortableRegistry, fields: &[Field<PortableForm>], depth: usize) -> String {
    fields
        .iter()
        .map(|field| {
            let ty = name(registry, field.ty.id, depth);
            match &field.name {
                Some(field_name) => format!("{field_name}: {ty}"),
                None => ty,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn definition(registry: &PortableRegistry, def: &TypeDef<PortableForm>, depth: usize) -> String {
    match def {
        TypeDef::Composite(composite) => {
            format!("{{{}}}", fields(registry, &composite.fields, depth))
        }
        TypeDef::Variant(variant) => variant
            .variants
            .iter()
            .map(|v| {
                format!(
                    "{}={}({})",
                    v.index,
                    v.name,
                    fields(registry, &v.fields, depth)
                )
            })
            .collect::<Vec<_>>()
            .join(" | "),
        _ => String::new(),
    }
}

fn variants(registry: &PortableRegistry, id: u32) -> Vec<String> {
    match registry.resolve(id).map(|ty| &ty.type_def) {
        Some(TypeDef::Variant(variant)) => variant
            .variants
            .iter()
            .map(|v| {
                format!(
                    "{:>3} {}({})",
                    v.index,
                    v.name,
                    fields(registry, &v.fields, 0)
                )
            })
            .collect(),
        Some(other) => vec![format!("non-variant {}", definition(registry, other, 0))],
        None => vec![format!("<missing type {id}>")],
    }
}

/// Render the surface as lines. With `include_types`, every named registry
/// type is appended with its structure, which catches layout changes hidden
/// behind an unchanged type name.
pub fn surface(metadata: &RuntimeMetadataV16, include_types: bool) -> Vec<String> {
    let registry = &metadata.types;
    let mut lines = Vec::new();
    for pallet in &metadata.pallets {
        let p = &pallet.name;
        lines.push(format!("pallet {p} index={}", pallet.index));
        if let Some(calls) = &pallet.calls {
            lines.extend(
                variants(registry, calls.ty.id)
                    .into_iter()
                    .map(|v| format!("  {p}.call {v}")),
            );
        }
        if let Some(event) = &pallet.event {
            lines.extend(
                variants(registry, event.ty.id)
                    .into_iter()
                    .map(|v| format!("  {p}.event {v}")),
            );
        }
        if let Some(error) = &pallet.error {
            lines.extend(
                variants(registry, error.ty.id)
                    .into_iter()
                    .map(|v| format!("  {p}.error {v}")),
            );
        }
        for constant in &pallet.constants {
            lines.push(format!(
                "  {p}.const {}: {} = 0x{}",
                constant.name,
                name(registry, constant.ty.id, 0),
                hex::encode(&constant.value)
            ));
        }
        if let Some(storage) = &pallet.storage {
            lines.push(format!("  {p}.storage-prefix {}", storage.prefix));
            for entry in &storage.entries {
                let ty = match &entry.ty {
                    StorageEntryType::Plain(value) => {
                        format!("plain {}", name(registry, value.id, 0))
                    }
                    StorageEntryType::Map {
                        hashers,
                        key,
                        value,
                    } => format!(
                        "map {hashers:?} {} => {}",
                        name(registry, key.id, 0),
                        name(registry, value.id, 0)
                    ),
                };
                lines.push(format!(
                    "  {p}.storage {} {:?} {ty} default=0x{}",
                    entry.name,
                    entry.modifier,
                    hex::encode(&entry.default)
                ));
            }
        }
        for associated in &pallet.associated_types {
            lines.push(format!(
                "  {p}.associated {} = {}",
                associated.name,
                name(registry, associated.ty.id, 0)
            ));
        }
        for view in &pallet.view_functions {
            lines.push(format!(
                "  {p}.view {} 0x{} ({}) -> {}",
                view.name,
                hex::encode(view.id),
                view.inputs
                    .iter()
                    .map(|input| format!("{}: {}", input.name, name(registry, input.ty.id, 0)))
                    .collect::<Vec<_>>()
                    .join(", "),
                name(registry, view.output.id, 0)
            ));
        }
    }
    for api in &metadata.apis {
        lines.push(format!("api {} version={}", api.name, api.version.0));
        for method in &api.methods {
            lines.push(format!(
                "  {}.{}({}) -> {}",
                api.name,
                method.name,
                method
                    .inputs
                    .iter()
                    .map(|input| format!("{}: {}", input.name, name(registry, input.ty.id, 0)))
                    .collect::<Vec<_>>()
                    .join(", "),
                name(registry, method.output.id, 0)
            ));
        }
    }
    let extrinsic = &metadata.extrinsic;
    lines.push(format!(
        "extrinsic versions={:?} address={} call={} signature={}",
        extrinsic.versions,
        name(registry, extrinsic.address_ty.id, 0),
        name(registry, extrinsic.call_ty.id, 0),
        name(registry, extrinsic.signature_ty.id, 0)
    ));
    for (version, indices) in &extrinsic.transaction_extensions_by_version {
        let indices: Vec<_> = indices.iter().map(|index| index.0).collect();
        lines.push(format!("extrinsic extensions v{version} = {indices:?}"));
    }
    for (index, extension) in extrinsic.transaction_extensions.iter().enumerate() {
        lines.push(format!(
            "extension {index} {} ty={} implicit={}",
            extension.identifier,
            name(registry, extension.ty.id, 0),
            name(registry, extension.implicit.id, 0)
        ));
    }
    let outer = &metadata.outer_enums;
    for (kind, id) in [
        ("call", outer.call_enum_ty.id),
        ("event", outer.event_enum_ty.id),
        ("error", outer.error_enum_ty.id),
    ] {
        lines.extend(
            variants(registry, id)
                .into_iter()
                .map(|v| format!("outer {kind} {v}")),
        );
    }
    for (key, value) in &metadata.custom.map {
        lines.push(format!(
            "custom {key} ty={} value=0x{}",
            name(registry, value.ty.id, 0),
            hex::encode(&value.value)
        ));
    }
    if include_types {
        let mut named: Vec<_> = registry
            .types
            .iter()
            .filter(|ty| !ty.ty.path.segments.is_empty())
            .map(|ty| {
                format!(
                    "type {} := {}",
                    name(registry, ty.id, 0),
                    definition(registry, &ty.ty.type_def, 0)
                )
            })
            .collect();
        named.sort();
        lines.extend(named);
    }
    lines
}

/// Surplus occurrences in `old` (prefixed `- `), then in `new` (prefixed `+ `),
/// each in its original order. Comparison counts duplicates but ignores line
/// ordering. An empty diff does not prove that the metadata blobs are equal.
pub fn diff(old: &[String], new: &[String]) -> Vec<String> {
    let mut delta = Vec::new();
    for (prefix, source, other) in [("-", old, new), ("+", new, old)] {
        let mut remaining = BTreeMap::new();
        for line in other {
            *remaining.entry(line).or_insert(0usize) += 1;
        }
        for line in source {
            if let Some(count) = remaining.get_mut(line) {
                if *count > 0 {
                    *count -= 1;
                    continue;
                }
            }
            delta.push(format!("{prefix} {line}"));
        }
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIP: &[u8] = include_bytes!("../../../tests/fixtures/native-2609/quip.scale");
    const TESTNET: &[u8] = include_bytes!("../../../tests/fixtures/native-2609/quip-testnet.scale");

    #[test]
    fn duplicate_occurrences_are_reported_individually() {
        let single = vec!["same line".into()];
        let double = vec!["same line".into(), "same line".into()];
        assert_eq!(diff(&single, &double), vec!["+ same line"]);
        assert_eq!(diff(&double, &single), vec!["- same line"]);
        assert_eq!(diff(&double, &[]), vec!["- same line", "- same line"]);
    }

    #[test]
    fn recursive_named_type_parameter_is_bounded() {
        let mut metadata = decode(QUIP).unwrap();
        let ty = metadata
            .types
            .types
            .iter_mut()
            .find(|ty| !ty.ty.path.segments.is_empty() && !ty.ty.type_params.is_empty())
            .unwrap();
        let id = ty.id;
        ty.ty.type_params[0].ty = Some(id.into());
        let rendered = name(&metadata.types, id, 0);
        assert!(rendered.contains(&format!("#{id}")));
    }

    #[test]
    fn identical_anchor_has_empty_diff() {
        let surface = surface(&decode(QUIP).unwrap(), true);
        assert!(surface.iter().any(|line| line == "pallet Vesting index=32"));
        assert!(diff(&surface, &surface).is_empty());
    }

    // The approved network differences: spec_name inside the version constant
    // and the epoch length. Runtime crate names are normalized first.
    #[test]
    fn networks_differ_only_in_approved_constants() {
        let quip = surface(&decode(QUIP).unwrap(), true);
        let testnet: Vec<_> = surface(&decode(TESTNET).unwrap(), true)
            .into_iter()
            .map(|line| line.replace("quip_testnet_runtime", "quip_protocol_runtime"))
            .collect();
        let delta = diff(&quip, &testnet);
        assert_eq!(delta.len(), 4, "{delta:#?}");
        for constant in ["System.const Version:", "Babe.const EpochDuration:"] {
            assert_eq!(
                delta.iter().filter(|line| line.contains(constant)).count(),
                2
            );
        }
    }

    #[test]
    fn rejects_trailing_bytes_and_garbage() {
        let mut padded = QUIP.to_vec();
        padded.push(0);
        assert_eq!(
            decode(&padded).unwrap_err(),
            "metadata has 1 trailing bytes"
        );
        assert!(decode(&[0, 1, 2]).is_err());
    }
}
