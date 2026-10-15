use std::collections::BTreeMap;
use std::ops::Range;

use crate::frontend::types::SourceTypeOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateInitializationError {
    pub(crate) path: String,
    pub(crate) range: Range<usize>,
    pub(crate) symbol: String,
    pub(crate) message: String,
}

impl StateInitializationError {
    pub(crate) fn to_source_diagnostic(&self) -> crate::SourceDiagnostic {
        crate::SourceDiagnostic::new(
            self.path.clone(),
            self.range.start,
            self.range.end,
            self.symbol.clone(),
            self.message.clone(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ScalarStorageKind {
    Bool,
    I32,
    U8,
    U16,
    U32,
    F32,
    F64,
}

impl ScalarStorageKind {
    /// Width of the canonical value encoding returned by
    /// [`TypedScalarDefault::to_le_bytes`]. This is distinct from a backend's
    /// physical binding width (for example, native bool state slots are 4
    /// bytes while the canonical bool payload is one byte).
    pub(crate) const fn byte_width(self) -> usize {
        match self {
            Self::Bool | Self::U8 => 1,
            Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StableDeclaredTypeIdentity {
    pub(crate) canonical_name: String,
    pub(crate) origin: Option<SourceTypeOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypedScalarDefault {
    pub(crate) declared_type: StableDeclaredTypeIdentity,
    pub(crate) storage_kind: ScalarStorageKind,
    /// Canonical value bits. Integer and bool values use only their storage
    /// width; floating-point values preserve their IEEE-754 bit patterns.
    pub(crate) bits: u64,
}

impl TypedScalarDefault {
    pub(crate) const fn byte_width(&self) -> usize {
        self.storage_kind.byte_width()
    }

    pub(crate) fn to_le_bytes(&self) -> Vec<u8> {
        let bytes = self.bits.to_le_bytes();
        let width = self.byte_width();
        debug_assert!(width <= bytes.len());
        debug_assert!(width == 8 || self.bits >> (width * 8) == 0);
        bytes[..width].to_vec()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StateInitializationPlan {
    pub(crate) scalar_defaults: BTreeMap<String, TypedScalarDefault>,
    pub(crate) collection_lane_defaults: BTreeMap<(String, String), TypedScalarDefault>,
}

impl StateInitializationPlan {
    pub(crate) fn scalar_default(&self, path: &str) -> Option<&TypedScalarDefault> {
        self.scalar_defaults.get(path)
    }

    pub(crate) fn collection_lane_default(
        &self,
        collection_path: &str,
        lane_path: &str,
    ) -> Option<&TypedScalarDefault> {
        self.collection_lane_defaults
            .get(&(collection_path.to_string(), lane_path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(storage_kind: ScalarStorageKind, bits: u64) -> TypedScalarDefault {
        TypedScalarDefault {
            declared_type: StableDeclaredTypeIdentity {
                canonical_name: "test".to_string(),
                origin: None,
            },
            storage_kind,
            bits,
        }
    }

    #[test]
    fn scalar_default_bytes_preserve_exact_width_and_bits() {
        assert_eq!(value(ScalarStorageKind::Bool, 1).to_le_bytes(), [1]);
        assert_eq!(
            value(ScalarStorageKind::I32, (-7_i32 as u32) as u64).to_le_bytes(),
            (-7_i32).to_le_bytes()
        );
        assert_eq!(
            value(ScalarStorageKind::U16, 0x1234).to_le_bytes(),
            [0x34, 0x12]
        );
        assert_eq!(
            value(
                ScalarStorageKind::F32,
                f32::from_bits(0x7fc0_1234).to_bits() as u64
            )
            .to_le_bytes(),
            0x7fc0_1234_u32.to_le_bytes()
        );
        assert_eq!(
            value(ScalarStorageKind::F64, 0x7ff8_0000_0000_1234).to_le_bytes(),
            0x7ff8_0000_0000_1234_u64.to_le_bytes()
        );
    }

    #[test]
    fn initialization_plan_lookups_are_separate_for_scalars_and_lanes() {
        let default = value(ScalarStorageKind::I32, 4);
        let plan = StateInitializationPlan {
            scalar_defaults: BTreeMap::from([("State.score".to_string(), default.clone())]),
            collection_lane_defaults: BTreeMap::from([(
                ("actors".to_string(), "stats.hp".to_string()),
                default.clone(),
            )]),
        };
        assert_eq!(plan.scalar_default("State.score"), Some(&default));
        assert_eq!(
            plan.collection_lane_default("actors", "stats.hp"),
            Some(&default)
        );
        assert!(plan.scalar_default("actors.stats.hp").is_none());
        assert!(plan.collection_lane_default("actors", "missing").is_none());
    }
}
