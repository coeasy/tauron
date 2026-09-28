//! Portable target contract for runtime/plugin artifacts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetOs {
    Windows,
    Macos,
    Linux,
    Ios,
    Android,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetArch {
    X86_64,
    Aarch64,
    X86,
    Armv7,
    Wasm32,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetAbi {
    Msvc,
    Gnu,
    Musl,
    Android,
    Apple,
    Wasi,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetSpec {
    pub os: TargetOs,
    pub arch: TargetArch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<TargetAbi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_os_version: Option<String>,
    #[serde(default)]
    pub cpu_features: Vec<String>,
}

impl TargetSpec {
    /// Conservative compatibility check used before an artifact reaches an OS loader.
    ///
    /// Unknown/minimum-version semantic ordering is intentionally not guessed here.
    /// A non-empty artifact minimum therefore requires the host to publish a concrete
    /// version string equal to it until a platform-specific comparator is installed.
    pub fn compatible_with(&self, host: &Self) -> bool {
        if self.os != host.os || self.arch != host.arch {
            return false;
        }
        if let Some(required_abi) = &self.abi {
            if host.abi.as_ref() != Some(required_abi) {
                return false;
            }
        }
        if let Some(required_version) = &self.min_os_version {
            if host.min_os_version.as_ref() != Some(required_version) {
                return false;
            }
        }
        self.cpu_features
            .iter()
            .all(|required| host.cpu_features.iter().any(|actual| actual == required))
    }

    pub fn specificity(&self) -> usize {
        usize::from(self.abi.is_some())
            + usize::from(self.min_os_version.is_some())
            + self.cpu_features.len()
    }
}

/// Pick the most-specific compatible artifact target.
pub fn resolve_best<'a>(variants: &'a [TargetSpec], host: &TargetSpec) -> Option<&'a TargetSpec> {
    variants
        .iter()
        .filter(|candidate| candidate.compatible_with(host))
        .max_by_key(|candidate| candidate.specificity())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> TargetSpec {
        TargetSpec {
            os: TargetOs::Linux,
            arch: TargetArch::X86_64,
            abi: Some(TargetAbi::Gnu),
            min_os_version: None,
            cpu_features: vec!["sse4.2".into(), "avx2".into()],
        }
    }

    #[test]
    fn rejects_wrong_arch_before_loader() {
        let artifact = TargetSpec { arch: TargetArch::Aarch64, ..host() };
        assert!(!artifact.compatible_with(&host()));
    }

    #[test]
    fn requires_declared_cpu_features() {
        let mut artifact = host();
        artifact.cpu_features = vec!["avx512f".into()];
        assert!(!artifact.compatible_with(&host()));
    }

    #[test]
    fn resolver_prefers_specific_compatible_variant() {
        let generic = TargetSpec {
            os: TargetOs::Linux,
            arch: TargetArch::X86_64,
            abi: None,
            min_os_version: None,
            cpu_features: vec![],
        };
        let exact = host();
        let variants = vec![generic, exact.clone()];
        assert_eq!(resolve_best(&variants, &host()), Some(&exact));
    }
}
