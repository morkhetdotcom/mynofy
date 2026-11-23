//! Immutable built-in catalog metadata. Parsed and checked once per process.
use crate::{ApiError, Result};
use serde_json::Value;
use std::{collections::BTreeSet, sync::OnceLock};

pub const CATALOG_NAME: &str = "mynofy";
pub const CATALOG_VERSION: u32 = 1;

/// Metadata retrieval does not enable typed-event publishing or install a catalog.
pub fn builtin_catalog() -> Result<&'static Value> {
    static CATALOG: OnceLock<Result<Value>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let value =
                crate::strict_json(include_str!("../../../protocol/catalogs/mynofy-v1.json"))?;
            if value["name"] != CATALOG_NAME || value["version"] != CATALOG_VERSION {
                return Err(ApiError::internal());
            }
            for group in ["categories", "actions", "statuses", "parameter_types"] {
                let entries = value[group].as_array().ok_or_else(ApiError::internal)?;
                let mut ids = BTreeSet::new();
                let mut names = BTreeSet::new();
                for entry in entries {
                    let id = entry["id"]
                        .as_u64()
                        .filter(|&n| n > 0)
                        .ok_or_else(ApiError::internal)?;
                    let name = entry["name"]
                        .as_str()
                        .filter(|n| !n.is_empty())
                        .ok_or_else(ApiError::internal)?;
                    if !ids.insert(id) || !names.insert(name) {
                        return Err(ApiError::internal());
                    }
                }
                if entries.is_empty() {
                    return Err(ApiError::internal());
                }
            }
            Ok(value)
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_is_complete_and_reused() {
        let c = builtin_catalog().unwrap();
        assert!(std::ptr::eq(c, builtin_catalog().unwrap()));
        assert_eq!(c["categories"].as_array().unwrap().len(), 43);
        assert_eq!(c["actions"].as_array().unwrap().len(), 49);
        assert_eq!(c["statuses"].as_array().unwrap().len(), 21);
        assert_eq!(c["parameter_types"].as_array().unwrap().len(), 7);
        assert_eq!(c["limits"]["max_opaque_payload_bytes"], 4096);
        for category in c["categories"].as_array().unwrap() {
            assert!(!category["name"].as_str().unwrap().starts_with("UNITY"));
            assert!(!(200..=203).contains(&category["id"].as_u64().unwrap()));
        }
    }
}
