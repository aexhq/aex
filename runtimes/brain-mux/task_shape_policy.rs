//! Shared build-time parser and policy for the `brain-mux` release task shape.

use serde::Deserialize;

const UNITS_SCHEMA: &str = "aex.units.v1";
const BRAIN_MUX_UNIT: &str = "brain-mux";
const REQUIRED_CPU: u32 = 2_048;
const REQUIRED_MEMORY_MIB: u32 = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct FargateShape {
    pub(crate) cpu: u32,
    pub(crate) memory_mb: u32,
}

#[derive(Deserialize)]
struct UnitsRegistry {
    schema: String,
    unit: Vec<ReleaseUnit>,
}

#[derive(Deserialize)]
struct ReleaseUnit {
    id: String,
    kind: String,
    package: String,
    fargate: Option<FargateShape>,
}

pub(crate) fn parse_brain_mux_shape(source: &str) -> Result<FargateShape, String> {
    let registry: UnitsRegistry = toml::from_str(source)
        .map_err(|error| format!("cannot parse release registry: {error}"))?;
    if registry.schema != UNITS_SCHEMA {
        return Err("release registry has an unsupported schema".to_owned());
    }
    let mut matches = registry
        .unit
        .into_iter()
        .filter(|unit| unit.id == BRAIN_MUX_UNIT);
    let unit = matches
        .next()
        .ok_or_else(|| "release registry must declare brain-mux exactly once".to_owned())?;
    if matches.next().is_some() {
        return Err("release registry declares brain-mux more than once".to_owned());
    }
    if unit.kind != "rust-oci-service" {
        return Err("brain-mux must remain a service-shaped OCI unit".to_owned());
    }
    if unit.package != BRAIN_MUX_UNIT {
        return Err("brain-mux release unit must build the brain-mux package".to_owned());
    }
    let shape = unit
        .fargate
        .ok_or_else(|| "brain-mux release unit must declare a Fargate shape".to_owned())?;
    if !valid_fargate_memory(shape.cpu, shape.memory_mb) {
        return Err("brain-mux CPU/memory is not a valid Fargate task shape".to_owned());
    }
    if shape.cpu != REQUIRED_CPU || shape.memory_mb != REQUIRED_MEMORY_MIB {
        return Err(format!(
            "brain-mux launch shape must remain exactly {REQUIRED_CPU} CPU units and \
             {REQUIRED_MEMORY_MIB} MiB until a measured capacity redesign lands"
        ));
    }
    Ok(shape)
}

const fn valid_fargate_memory(cpu: u32, memory_mib: u32) -> bool {
    match cpu {
        1_024 => memory_mib >= 2_048 && memory_mib <= 8_192 && memory_mib.is_multiple_of(1_024),
        2_048 => memory_mib >= 4_096 && memory_mib <= 16_384 && memory_mib.is_multiple_of(1_024),
        4_096 => memory_mib >= 8_192 && memory_mib <= 30_720 && memory_mib.is_multiple_of(1_024),
        8_192 => memory_mib >= 16_384 && memory_mib <= 61_440 && memory_mib.is_multiple_of(4_096),
        16_384 => memory_mib >= 32_768 && memory_mib <= 122_880 && memory_mib.is_multiple_of(8_192),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{FargateShape, parse_brain_mux_shape};

    fn registry(rows: &str) -> String {
        format!("schema = \"aex.units.v1\"\n{rows}")
    }

    fn row(cpu: u32, memory_mib: u32) -> String {
        format!(
            "[[unit]]\n\
             id = \"brain-mux\"\n\
             kind = \"rust-oci-service\"\n\
             package = \"brain-mux\"\n\
             [unit.fargate]\n\
             cpu = {cpu}\n\
             memory_mb = {memory_mib}\n"
        )
    }

    #[test]
    fn exact_launch_shape_is_accepted() {
        assert_eq!(
            parse_brain_mux_shape(&registry(&row(2_048, 4_096))),
            Ok(FargateShape {
                cpu: 2_048,
                memory_mb: 4_096,
            })
        );
    }

    #[test]
    fn alternate_valid_fargate_shapes_are_rejected() {
        for (cpu, memory_mib) in [(1_024, 2_048), (2_048, 8_192), (4_096, 8_192)] {
            let error = parse_brain_mux_shape(&registry(&row(cpu, memory_mib)))
                .expect_err("valid Fargate is not sufficient capacity evidence");
            assert!(error.contains("must remain exactly"), "{error}");
        }
    }

    #[test]
    fn missing_duplicate_and_invalid_shapes_fail_closed() {
        assert!(
            parse_brain_mux_shape(&registry("")).is_err(),
            "a missing unit cannot inherit defaults"
        );
        let duplicate = format!("{}{}", row(2_048, 4_096), row(2_048, 4_096));
        assert!(
            parse_brain_mux_shape(&registry(&duplicate))
                .expect_err("duplicate unit")
                .contains("more than once")
        );
        assert!(
            parse_brain_mux_shape(&registry(&row(2_048, 2_048)))
                .expect_err("invalid Fargate shape")
                .contains("not a valid Fargate")
        );
    }
}
