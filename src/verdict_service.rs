// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Where Arksa's hosted verdict service is and how Arksa authenticates to it (owner decision,
//! 20 November 2026, roadmap 0.42 §1i).
//!
//! The hosted methods arksa-kpro uses (simulator recalibration, `simcal recalibrate`, and
//! lunar GNSS assistance packages, `lunar-time assist request`) run in the hosted verdict
//! service, never in this crate. In Arksa's cloud that service runs as an internal service
//! next to Arksa, on the private network (its `arksa-internal` profile); a Sovereign install
//! calls a contracted instance beside it (`sovereign-contracted`). Either way Arksa holds ONE
//! service credential, not per-customer licences, and tags every call with the tenant or
//! mission it is for, so the service meters cost per tenant.
//!
//! What a call needs, resolved by [`resolve`] (first match wins, nothing is ever read from the
//! repository):
//!
//! | | command-line flag | environment | fallback |
//! |---|---|---|---|
//! | base URL | `--service` | `ARKSA_VERDICT_URL` | `KSHANA_VERDICT_URL` |
//! | service credential | `--verdict-license` | `ARKSA_VERDICT_CREDENTIAL_FILE` (a file holding it), `ARKSA_VERDICT_CREDENTIAL` | `KSHANA_VERDICT_LICENSE`; the command's older flag |
//! | trusted verdict key(s) | `--trusted-key` | `ARKSA_VERDICT_KEY` | `KSHANA_VERDICT_KEY`, then the key built into the release |
//! | tenant/mission tag | `--tenant` | `ARKSA_TENANT` | the command's own default (the mission file's `tenant`, then its `mission` id) |
//! | internal CA certificate | `--verdict-ca` | `ARKSA_VERDICT_CA` | the system trust store |
//!
//! The URL must be `https://` (an internal certificate authority via the CA file) or a
//! loopback `http://` address (a sidecar on the same host). The tag is 1 to 64 characters of
//! `A-Z a-z 0-9 . _ -` and travels in the `X-Meru-Tenant` header.

use kshana_pro::remote_verdict::VerdictTransport;

/// The header that carries the tenant/mission tag.
pub const TENANT_HEADER: &str = "X-Meru-Tenant";
/// The longest tenant tag the service accepts.
pub const TENANT_MAX: usize = 64;

/// What a call to the hosted verdict service needs.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// Base URL (`https://meru.internal.example` or `http://127.0.0.1:8099`).
    pub url: String,
    /// The service credential (a vendor-signed token). Never printed.
    pub credential: String,
    /// The service's verdict public key(s), 64 hex each, comma separated.
    pub trusted_keys: String,
    /// The tenant/mission tag sent with every call.
    pub tenant: String,
    /// A CA certificate file for an internal TLS endpoint.
    pub ca_file: Option<String>,
}

impl std::fmt::Debug for ServiceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceConfig")
            .field("url", &self.url)
            .field("credential", &"<redacted>")
            .field("trusted_keys", &self.trusted_keys)
            .field("tenant", &self.tenant)
            .field("ca_file", &self.ca_file)
            .finish()
    }
}

/// The command's own names and defaults for [`resolve`].
#[derive(Default)]
pub struct Defaults<'a> {
    /// An older flag that also carries the credential (`lunar-time assist request --license`).
    pub legacy_credential_flag: Option<&'a str>,
    /// Tenant tags to fall back on, in order (for `simcal`: the mission file's `tenant`, then
    /// its `mission` id); the first valid one is used.
    pub tenants: Vec<String>,
    /// Whether to read the older `KSHANA_VERDICT_*` variables.
    pub legacy_env: bool,
}

/// Check a tenant tag (1 to [`TENANT_MAX`] characters of `[A-Za-z0-9._-]`).
pub fn validate_tenant(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag.len() > TENANT_MAX {
        return Err(format!("tenant tag must be 1 to {TENANT_MAX} characters"));
    }
    if !tag
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "tenant tag '{tag}' may hold only A-Z, a-z, 0-9, '.', '_' and '-'"
        ));
    }
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    kshana_pro::cli::flag(args, name).filter(|s| !s.trim().is_empty())
}

/// Resolve the service configuration from the command line `args`, the environment `env`
/// (a lookup, so tests need not touch the process environment) and the command's
/// [`Defaults`]. See the module docs for the order.
pub fn resolve(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    d: &Defaults<'_>,
) -> Result<ServiceConfig, String> {
    let env = |k: &str| env(k).filter(|s| !s.trim().is_empty());
    let legacy = |k: &str| if d.legacy_env { env(k) } else { None };
    let url = flag(args, "--service")
        .or_else(|| env("ARKSA_VERDICT_URL"))
        .or_else(|| legacy("KSHANA_VERDICT_URL"))
        .ok_or("the verdict service is not configured: --service <url> or ARKSA_VERDICT_URL")?;
    check_url(&url)?;
    let credential = match flag(args, "--verdict-license") {
        Some(c) => c,
        None => match env("ARKSA_VERDICT_CREDENTIAL_FILE") {
            Some(f) => std::fs::read_to_string(&f)
                .map_err(|e| format!("ARKSA_VERDICT_CREDENTIAL_FILE {f}: {e}"))?
                .trim()
                .to_string(),
            None => env("ARKSA_VERDICT_CREDENTIAL")
                .or_else(|| legacy("KSHANA_VERDICT_LICENSE"))
                .or_else(|| d.legacy_credential_flag.and_then(|f| flag(args, f)))
                .ok_or(
                    "the service credential is not configured: ARKSA_VERDICT_CREDENTIAL_FILE, \
                     ARKSA_VERDICT_CREDENTIAL or --verdict-license",
                )?,
        },
    };
    if credential.trim().is_empty() || credential.contains(['\n', '\r', '"', '\\']) {
        return Err("the service credential is empty or holds invalid characters".into());
    }
    let trusted_keys = flag(args, "--trusted-key")
        .or_else(|| env("ARKSA_VERDICT_KEY"))
        .or_else(|| legacy("KSHANA_VERDICT_KEY"))
        .or_else(|| kshana_pro::remote_verdict::builtin_trusted_keys().map(str::to_string))
        .ok_or(
            "the service's verdict key is not configured: --trusted-key <hex> or ARKSA_VERDICT_KEY",
        )?;
    for k in trusted_keys.split(',').map(str::trim) {
        if k.len() != 64 || hex::decode(k).is_err() {
            return Err(format!(
                "trusted verdict key '{k}' is not 64 hex characters"
            ));
        }
    }
    let tenant = match flag(args, "--tenant").or_else(|| env("ARKSA_TENANT")) {
        Some(t) => {
            validate_tenant(&t)?;
            t
        }
        None => d
            .tenants
            .iter()
            .find(|t| validate_tenant(t).is_ok())
            .cloned()
            .ok_or(
                "no tenant/mission tag: --tenant <tag> or ARKSA_TENANT (1-64 of A-Z a-z 0-9 . _ -)",
            )?,
    };
    let ca_file = flag(args, "--verdict-ca").or_else(|| env("ARKSA_VERDICT_CA"));
    if let Some(ca) = &ca_file {
        if !std::path::Path::new(ca).is_file() || ca.contains(['\n', '\r', '"', '\\']) {
            return Err(format!("verdict CA file '{ca}' is not a readable file"));
        }
    }
    Ok(ServiceConfig {
        url,
        credential: credential.trim().to_string(),
        trusted_keys,
        tenant,
        ca_file,
    })
}

/// The service URL must be `https://`, or `http://` on loopback (a sidecar on the same host).
pub fn check_url(url: &str) -> Result<(), String> {
    let ok = url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
        || url.starts_with("http://[::1]");
    if !ok || url.contains(['\n', '\r', '"', '\\', ' ']) {
        return Err(format!(
            "verdict service URL '{url}' must be https:// (internal CA via ARKSA_VERDICT_CA) or a loopback http:// address"
        ));
    }
    Ok(())
}

impl ServiceConfig {
    /// The transport for this configuration.
    pub fn transport(&self) -> TaggedTransport {
        TaggedTransport {
            base_url: self.url.clone(),
            tenant: self.tenant.clone(),
            ca_file: self.ca_file.clone(),
        }
    }
}

/// Kshana Pro's verdict transport with the tenant tag: the system `curl`, HTTPS or loopback
/// HTTP; the credential and the headers go to curl on stdin (never on the command line,
/// where other users could read them), the body through a temporary file.
pub struct TaggedTransport {
    pub base_url: String,
    pub tenant: String,
    pub ca_file: Option<String>,
}

impl VerdictTransport for TaggedTransport {
    fn post(&self, path: &str, bearer: &str, body: &str) -> Result<String, String> {
        use std::io::Write;
        let base = self.base_url.trim_end_matches('/');
        check_url(base)?;
        validate_tenant(&self.tenant)?;
        if bearer.contains(['\n', '\r', '"', '\\']) {
            return Err("service credential contains invalid characters".into());
        }
        let tmp = std::env::temp_dir().join(format!(
            "arksa-verdict-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let mut child = std::process::Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "600",
                "--config",
                "-",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                &format!("@{}", tmp.display()),
                "-w",
                "\n%{http_code}",
                &format!("{base}{path}"),
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "header = \"Authorization: Bearer {bearer}\"");
            let _ = writeln!(stdin, "header = \"{TENANT_HEADER}: {}\"", self.tenant);
            if let Some(ca) = &self.ca_file {
                let _ = writeln!(stdin, "cacert = \"{ca}\"");
            }
        }
        let out = child.wait_with_output();
        let _ = std::fs::remove_file(&tmp);
        let out = out.map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (resp, code) = text.rsplit_once('\n').unwrap_or((text.as_str(), "0"));
        match code.trim().parse::<u16>().unwrap_or(0) {
            200..=299 => Ok(resp.to_string()),
            c => {
                let msg = serde_json::from_str::<serde_json::Value>(resp)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                    .unwrap_or_else(|| resp.trim().to_string());
                Err(format!("verdict service answered {c}: {msg}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const K1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const K2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn envs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn environment_configures_the_internal_service() {
        let e = envs(&[
            ("ARKSA_VERDICT_URL", "https://meru.internal.example"),
            ("ARKSA_VERDICT_CREDENTIAL", "svc-token"),
            ("ARKSA_VERDICT_KEY", K1),
            ("ARKSA_TENANT", "tenant-a"),
        ]);
        let c = resolve(&[], &e, &Defaults::default()).unwrap();
        assert_eq!(c.url, "https://meru.internal.example");
        assert_eq!(c.credential, "svc-token");
        assert_eq!(c.trusted_keys, K1);
        assert_eq!(c.tenant, "tenant-a");
        assert_eq!(c.ca_file, None);
        // The credential never shows in debug output.
        assert!(!format!("{c:?}").contains("svc-token"));
    }

    #[test]
    fn flags_win_then_arksa_env_then_legacy_env() {
        let e = envs(&[
            ("ARKSA_VERDICT_URL", "https://a.example"),
            ("KSHANA_VERDICT_URL", "https://k.example"),
            ("KSHANA_VERDICT_LICENSE", "legacy-token"),
            ("KSHANA_VERDICT_KEY", K2),
            ("ARKSA_TENANT", "env-tenant"),
        ]);
        let d = Defaults {
            legacy_env: true,
            tenants: vec!["MISSION-1".into()],
            ..Default::default()
        };
        let c = resolve(&[], &e, &d).unwrap();
        assert_eq!(c.url, "https://a.example");
        assert_eq!(c.credential, "legacy-token");
        assert_eq!(c.trusted_keys, K2);
        assert_eq!(c.tenant, "env-tenant");
        let c = resolve(
            &args(&[
                "--service",
                "http://127.0.0.1:9",
                "--verdict-license",
                "flag-token",
                "--trusted-key",
                K1,
                "--tenant",
                "flag.tenant",
            ]),
            &e,
            &d,
        )
        .unwrap();
        assert_eq!(
            (
                c.url.as_str(),
                c.credential.as_str(),
                c.trusted_keys.as_str(),
                c.tenant.as_str()
            ),
            ("http://127.0.0.1:9", "flag-token", K1, "flag.tenant")
        );
        // Without the legacy switch the KSHANA_* variables are not read.
        let only_legacy = envs(&[("KSHANA_VERDICT_URL", "https://k.example")]);
        assert!(resolve(&[], &only_legacy, &Defaults::default())
            .unwrap_err()
            .contains("ARKSA_VERDICT_URL"));
    }

    #[test]
    fn credential_file_and_ca_file() {
        let dir = std::env::temp_dir().join(format!("ak-vs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cred = dir.join("cred");
        std::fs::write(&cred, "file-token\n").unwrap();
        let ca = dir.join("ca.pem");
        std::fs::write(&ca, "-----BEGIN CERTIFICATE-----\n").unwrap();
        let e = envs(&[
            ("ARKSA_VERDICT_URL", "https://meru.internal.example"),
            ("ARKSA_VERDICT_CREDENTIAL_FILE", cred.to_str().unwrap()),
            ("ARKSA_VERDICT_CREDENTIAL", "env-token"),
            ("ARKSA_VERDICT_KEY", &format!("{K1},{K2}")),
            ("ARKSA_VERDICT_CA", ca.to_str().unwrap()),
            ("ARKSA_TENANT", "t"),
        ]);
        let c = resolve(&[], &e, &Defaults::default()).unwrap();
        assert_eq!(
            c.credential, "file-token",
            "the file wins over the variable"
        );
        assert_eq!(c.ca_file.as_deref(), ca.to_str());
        let e = envs(&[
            ("ARKSA_VERDICT_URL", "https://x.example"),
            ("ARKSA_VERDICT_CREDENTIAL_FILE", "/nonexistent/cred"),
            ("ARKSA_VERDICT_KEY", K1),
            ("ARKSA_TENANT", "t"),
        ]);
        assert!(resolve(&[], &e, &Defaults::default())
            .unwrap_err()
            .contains("ARKSA_VERDICT_CREDENTIAL_FILE"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tenant_defaults_and_validation() {
        let base = [
            ("ARKSA_VERDICT_URL", "https://x.example"),
            ("ARKSA_VERDICT_CREDENTIAL", "tok"),
            ("ARKSA_VERDICT_KEY", K1),
        ];
        let e = envs(&base);
        // The first valid default: a mission id with a space is skipped.
        let d = Defaults {
            tenants: vec!["Toy mission".into(), "TOY-1".into()],
            ..Default::default()
        };
        assert_eq!(resolve(&[], &e, &d).unwrap().tenant, "TOY-1");
        assert!(resolve(&[], &e, &Defaults::default())
            .unwrap_err()
            .contains("tenant"));
        assert!(resolve(&args(&["--tenant", "a/b"]), &e, &d)
            .unwrap_err()
            .contains("may hold only"));
        for bad in ["", "a b", "x\"y", &"x".repeat(65)] {
            assert!(validate_tenant(bad).is_err(), "{bad:?}");
        }
        assert!(validate_tenant(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn urls_keys_and_credentials_are_checked() {
        let e = |url: &str, key: &str, cred: &str| {
            envs(&[
                ("ARKSA_VERDICT_URL", url),
                ("ARKSA_VERDICT_CREDENTIAL", cred),
                ("ARKSA_VERDICT_KEY", key),
                ("ARKSA_TENANT", "t"),
            ])
        };
        let d = Defaults::default();
        for bad in ["http://10.0.0.5:8099", "ftp://x", "meru:8099"] {
            assert!(resolve(&[], &e(bad, K1, "tok"), &d)
                .unwrap_err()
                .contains("https://"));
        }
        for ok in [
            "https://10.0.0.5",
            "http://127.0.0.1:8099",
            "http://[::1]:8099",
        ] {
            assert!(resolve(&[], &e(ok, K1, "tok"), &d).is_ok(), "{ok}");
        }
        assert!(resolve(&[], &e("https://x", "abc", "tok"), &d)
            .unwrap_err()
            .contains("64 hex"));
        assert!(resolve(&[], &e("https://x", K1, "a\"b"), &d)
            .unwrap_err()
            .contains("invalid characters"));
        // The older per-command flag still carries the credential.
        let no_cred = envs(&[
            ("ARKSA_VERDICT_URL", "https://x"),
            ("ARKSA_VERDICT_KEY", K1),
            ("ARKSA_TENANT", "t"),
        ]);
        let d = Defaults {
            legacy_credential_flag: Some("--license"),
            ..Default::default()
        };
        assert_eq!(
            resolve(&args(&["--license", "old"]), &no_cred, &d)
                .unwrap()
                .credential,
            "old"
        );
    }
}
