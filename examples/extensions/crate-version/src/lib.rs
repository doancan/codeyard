//! Crate Version: a command that fetches. Requests go through Codeyard, which checks the host against `net` in the
//! manifest; a refused request comes back as an error that says what to add.

use codeyard_extension::prelude::*;

struct CrateVersion;

impl Extension for CrateVersion {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("latest", async |ctx, _args| {
            let Some(name) = ctx.input(InputOptions::prompt("Crate name")).await?.filter(|n| !n.trim().is_empty()) else {
                return Ok(Value::Null);
            };
            let url = format!("https://crates.io/api/v1/crates/{}", name.trim());
            // crates.io asks every client to say who it is.
            let res = ctx.fetch(Request::get(url).header("user-agent", "codeyard-crate-version-example")).await?;
            if res.status == 404 {
                ctx.notice(format!("There is no crate named {name} on crates.io."));
                return Ok(Value::Null);
            }
            let body: Value = res.error_for_status()?.json()?;
            let version = body["crate"]["max_stable_version"].as_str().or(body["crate"]["max_version"].as_str()).unwrap_or("?").to_string();
            ctx.notice(format!("{name} {version}"));
            Ok(json!(version))
        });
        Ok(())
    }
}

codeyard_extension::export!(CrateVersion);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn shows_the_latest_version() {
        let host = MockHost::new();
        host.http("GET", "https://crates.io/api/v1/crates/serde", 200, r#"{"crate":{"max_version":"2.0.0-rc.1","max_stable_version":"1.0.228"}}"#);
        host.http("GET", "https://crates.io/api/v1/crates/nope", 404, "{}");
        host.activate::<CrateVersion>().unwrap();
        host.answer_input("serde");
        assert_eq!(host.command("latest", Value::Null).unwrap(), json!("1.0.228"));
        assert_eq!(host.fetched()[0].headers, [("user-agent".to_string(), "codeyard-crate-version-example".to_string())]);
        host.answer_input("nope");
        assert_eq!(host.command("latest", Value::Null).unwrap(), Value::Null);
        assert_eq!(host.notices(), ["serde 1.0.228", "There is no crate named nope on crates.io."]);
    }
}
