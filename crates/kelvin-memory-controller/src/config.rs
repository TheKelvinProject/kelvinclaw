use jsonwebtoken::DecodingKey;
use kelvin_core::{KelvinError, KelvinResult};
use tonic::transport::{Certificate, Identity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProfile {
    Minimal,
    IPhone,
    LinuxGpu,
}

#[derive(Debug, Clone)]
pub struct MemoryControllerConfig {
    pub issuer: String,
    pub audience: String,
    pub decoding_key_pem: String,
    pub decoding_key_path: String,
    pub tls_cert_pem: String,
    pub tls_cert_path: String,
    pub tls_key_pem: String,
    pub tls_key_path: String,
    pub tls_client_ca_pem: String,
    pub tls_client_ca_path: String,
    pub clock_skew_secs: u64,
    pub max_module_bytes: usize,
    pub max_memory_pages: u32,
    pub default_fuel: u64,
    pub default_timeout_ms: u64,
    pub default_max_response_bytes: usize,
    pub replay_window_secs: u64,
    pub profile: ProviderProfile,
}

impl Default for MemoryControllerConfig {
    fn default() -> Self {
        Self {
            issuer: "kelvin-root".to_string(),
            audience: "kelvin-memory-controller".to_string(),
            decoding_key_pem: String::new(),
            decoding_key_path: String::new(),
            tls_cert_pem: String::new(),
            tls_cert_path: String::new(),
            tls_key_pem: String::new(),
            tls_key_path: String::new(),
            tls_client_ca_pem: String::new(),
            tls_client_ca_path: String::new(),
            clock_skew_secs: 30,
            max_module_bytes: 2 * 1024 * 1024,
            max_memory_pages: 64,
            default_fuel: 100_000,
            default_timeout_ms: 2_000,
            default_max_response_bytes: 1024 * 1024,
            replay_window_secs: 120,
            profile: ProviderProfile::Minimal,
        }
    }
}

impl MemoryControllerConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(value) = std::env::var("KELVIN_MEMORY_ISSUER") {
            if !value.trim().is_empty() {
                cfg.issuer = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_AUDIENCE") {
            if !value.trim().is_empty() {
                cfg.audience = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_PUBLIC_KEY_PEM") {
            if !value.trim().is_empty() {
                cfg.decoding_key_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_PUBLIC_KEY_PATH") {
            if !value.trim().is_empty() {
                cfg.decoding_key_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_CERT_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_cert_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_CERT_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_cert_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_KEY_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_key_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_KEY_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_key_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_CLIENT_CA_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_client_ca_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TLS_CLIENT_CA_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_client_ca_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_CLOCK_SKEW_SECS") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.clock_skew_secs = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_MAX_MODULE_BYTES") {
            if let Ok(parsed) = value.parse::<usize>() {
                cfg.max_module_bytes = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_MAX_MEMORY_PAGES") {
            if let Ok(parsed) = value.parse::<u32>() {
                cfg.max_memory_pages = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_DEFAULT_FUEL") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.default_fuel = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_DEFAULT_TIMEOUT_MS") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.default_timeout_ms = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_DEFAULT_MAX_RESPONSE_BYTES") {
            if let Ok(parsed) = value.parse::<usize>() {
                cfg.default_max_response_bytes = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_REPLAY_WINDOW_SECS") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.replay_window_secs = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_PROFILE") {
            let normalized = value.trim().to_ascii_lowercase();
            cfg.profile = match normalized.as_str() {
                "iphone" => ProviderProfile::IPhone,
                "linux-gpu" | "linux_gpu" => ProviderProfile::LinuxGpu,
                _ => ProviderProfile::Minimal,
            };
        }
        cfg
    }

    pub fn validate(&self) -> KelvinResult<()> {
        validate_non_empty("memory issuer", &self.issuer)?;
        validate_non_empty("memory audience", &self.audience)?;
        if self.clock_skew_secs > 300 {
            return Err(KelvinError::InvalidInput(
                "memory clock skew must be <= 300 seconds".to_string(),
            ));
        }
        if self.max_module_bytes == 0 {
            return Err(KelvinError::InvalidInput(
                "memory max module bytes must be > 0".to_string(),
            ));
        }
        if self.max_memory_pages == 0 {
            return Err(KelvinError::InvalidInput(
                "memory max memory pages must be > 0".to_string(),
            ));
        }
        if self.default_fuel == 0 {
            return Err(KelvinError::InvalidInput(
                "memory default fuel must be > 0".to_string(),
            ));
        }
        if self.default_timeout_ms == 0 {
            return Err(KelvinError::InvalidInput(
                "memory default timeout must be > 0".to_string(),
            ));
        }
        if self.default_max_response_bytes == 0 {
            return Err(KelvinError::InvalidInput(
                "memory max response bytes must be > 0".to_string(),
            ));
        }
        if self.replay_window_secs > 3_600 {
            return Err(KelvinError::InvalidInput(
                "memory replay window must be <= 3600 seconds".to_string(),
            ));
        }
        Ok(())
    }

    pub fn decoding_key(&self) -> KelvinResult<DecodingKey> {
        let pem = resolve_required_pem(
            &self.decoding_key_pem,
            &self.decoding_key_path,
            "memory controller public key",
        )?;
        DecodingKey::from_ed_pem(pem.as_bytes())
            .map_err(|err| KelvinError::InvalidInput(format!("invalid decoding key pem: {err}")))
    }

    pub fn tls_identity(&self) -> KelvinResult<Option<Identity>> {
        let cert = resolve_optional_pem(
            &self.tls_cert_pem,
            &self.tls_cert_path,
            "memory controller tls cert",
        )?;
        let key = resolve_optional_pem(
            &self.tls_key_pem,
            &self.tls_key_path,
            "memory controller tls key",
        )?;
        match (cert, key) {
            (Some(cert), Some(key)) => Ok(Some(Identity::from_pem(cert, key))),
            (None, None) => Ok(None),
            _ => Err(KelvinError::InvalidInput(
                "controller tls requires both cert and key".to_string(),
            )),
        }
    }

    pub fn tls_client_ca(&self) -> KelvinResult<Option<Certificate>> {
        Ok(resolve_optional_pem(
            &self.tls_client_ca_pem,
            &self.tls_client_ca_path,
            "memory controller tls client ca",
        )?
        .map(Certificate::from_pem))
    }
}

fn resolve_required_pem(inline: &str, path: &str, label: &str) -> KelvinResult<String> {
    resolve_optional_pem(inline, path, label)?.ok_or_else(|| {
        KelvinError::InvalidInput(format!("{label} must be provided via inline pem or path"))
    })
}

fn validate_non_empty(label: &str, value: &str) -> KelvinResult<()> {
    if value.trim().is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MemoryControllerConfig;

    #[test]
    fn config_validate_rejects_zero_timeout() {
        let mut cfg = MemoryControllerConfig::default();
        cfg.default_timeout_ms = 0;
        let err = cfg.validate().expect_err("zero timeout should fail");
        assert!(err.to_string().contains("default timeout"));
    }

    #[test]
    fn config_validate_rejects_issuer_whitespace() {
        let mut cfg = MemoryControllerConfig::default();
        cfg.issuer = "   ".to_string();
        let err = cfg.validate().expect_err("empty issuer should fail");
        assert!(err.to_string().contains("memory issuer"));
    }
}

fn resolve_optional_pem(inline: &str, path: &str, label: &str) -> KelvinResult<Option<String>> {
    let inline = inline.trim();
    let path = path.trim();
    if !inline.is_empty() && !path.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} cannot set both inline pem and path"
        )));
    }
    if !inline.is_empty() {
        return Ok(Some(inline.to_string()));
    }
    if path.is_empty() {
        return Ok(None);
    }
    let pem = std::fs::read_to_string(path).map_err(|err| {
        KelvinError::InvalidInput(format!("{label} path '{path}' is not readable: {err}"))
    })?;
    if pem.trim().is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} path '{path}' is empty"
        )));
    }
    Ok(Some(pem))
}
