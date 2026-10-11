//! Intentional credential output is limited to enrollment links, reset codes and newly created PATs.
use crate::{
    Command,
    client::{Error, Result, field},
};
use qrcode::{QrCode, render::unicode::Dense1x2};
use serde_json::Value;

fn text(value: &Value, name: &str) -> String {
    value[name]
        .as_str()
        .unwrap_or("-")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn escaped_body(value: &Value) -> String {
    value["body"]
        .as_str()
        .unwrap_or("-")
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
pub fn print(command: &Command, value: &Value, json_output: bool) -> Result<()> {
    if matches!(command, Command::Export(_)) && value["stdout"] == true {
        return Ok(());
    }
    if json_output {
        println!("{value}");
        return Ok(());
    }
    match command {
        Command::Export(_) => println!(
            "Exported {} bytes to {}",
            value["bytes"],
            value["out"].as_str().unwrap_or("stdout")
        ),
        Command::Release { .. } => println!(
            "Running: {}\nLatest: {}\nUpdate available: {}\nChecked at: {}\nLast error: {}\nClient release: {} (Android {})\nUpgrade command: {}",
            text(value, "running"),
            text(&value["latest"], "version"),
            value["update_available"],
            text(value, "checked_at"),
            text(value, "last_error"),
            text(&value["client_release"], "version"),
            value["client_release"]["android_version_code"],
            text(value, "upgrade_command")
        ),
        Command::SecurityEvents { .. } => {
            println!(
                "OCCURRED AT\tKIND\tOUTCOME\tTARGET\tACTOR MEMBER\tACTOR DEVICE\tACTOR TOKEN\tREQUEST"
            );
            if let Some(items) = value["items"].as_array() {
                for e in items {
                    println!(
                        "{}\t{}\t{}\t{}:{}\t{}\t{}\t{}\t{}",
                        text(e, "occurred_at"),
                        text(e, "kind"),
                        text(e, "outcome"),
                        text(e, "target_type"),
                        text(e, "target_id"),
                        text(&e["actor"], "member_id"),
                        text(&e["actor"], "device_id"),
                        text(&e["actor"], "token_id"),
                        text(&e["actor"], "request_id")
                    );
                }
            }
        }
        Command::Backups => println!(
            "Last success: {}\nNext due: {}\nLast error: {}",
            text(value, "last_success"),
            text(value, "next_due"),
            text(value, "last_error")
        ),
        Command::Notifications { .. } => {
            if let Some(items) = value["items"].as_array() {
                println!("ID\tCATEGORY\tSUMMARY\tREAD");
                for n in items {
                    println!(
                        "{}\t{}\t{}\t{}",
                        text(n, "id"),
                        text(n, "category"),
                        text(&n["summary"], "text"),
                        !n["read_at"].is_null()
                    );
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(value)
                        .map_err(|_| Error::Local("Invalid notification response"))?
                );
            }
        }
        Command::Comments { .. } => {
            if let Some(rows) = value["items"].as_array() {
                for row in rows {
                    println!("{}\t{}", text(row, "id"), escaped_body(row));
                }
            } else {
                println!("{}\t{}", text(value, "id"), escaped_body(value));
            }
        }
        Command::Attach { .. } => {
            println!("{}\t{}", text(value, "id"), text(value, "original_name"))
        }
        Command::Import { .. } => println!(
            "created {} updated {} unchanged {} removed {} skipped {} ({} warnings)",
            value["created"],
            value["updated"],
            value["unchanged"],
            value["removed"],
            value["skipped"],
            value["warnings"].as_array().map_or(0, Vec::len)
        ),
        Command::Feed { .. } => {
            if let Some(items) = value["items"].as_array() {
                println!("ID\tNAME\tVERSION\tSTATUS");
                for item in items {
                    println!(
                        "{}\t{}\t{}\t{}",
                        text(item, "id"),
                        text(item, "name"),
                        item["version"],
                        text(item, "last_status")
                    );
                }
                if let Some(cursor) = value["next_cursor"].as_str() {
                    println!("next_cursor: {cursor}");
                }
            } else {
                println!("{} version {}", text(value, "id"), value["version"]);
            }
        }
        Command::Schedule { .. } => println!(
            "{}",
            serde_json::to_string_pretty(value)
                .map_err(|_| Error::Local("Invalid planning response"))?
        ),
        Command::Lists { .. } => crate::lists::print(value),
        Command::Enroll { .. } => {
            let uri = field(value, "uri")?;
            let qr = QrCode::new(uri.as_bytes())
                .map_err(|_| Error::Local("Could not render enrollment QR"))?;
            println!(
                "{uri}\n{}\nexpires at {}",
                qr.render::<Dense1x2>().quiet_zone(true).build(),
                field(value, "expires_at")?
            );
        }
        Command::Whoami => println!(
            "{} ({})\nscopes: {}\nexpires at {}",
            text(value, "username"),
            text(value, "name"),
            value["scopes"]
                .as_array()
                .map(|s| s
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            text(value, "expires_at")
        ),
        Command::User {
            command: crate::users::Command::ResetPassword { .. },
        } => println!(
            "{}\nexpires at {}",
            field(value, "code")?,
            field(value, "expires_at")?
        ),
        Command::Device {
            command: crate::DeviceCommand::List { .. },
        } => {
            println!("ID\tUSER\tLABEL\tPLATFORM\tREVOKED\tLAST SEEN");
            for device in value["devices"]
                .as_array()
                .ok_or(Error::Local("Invalid device list"))?
            {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    text(device, "id"),
                    text(device, "username"),
                    text(device, "label"),
                    text(device, "platform"),
                    if device["revoked_at"].is_null() {
                        "no"
                    } else {
                        "yes"
                    },
                    text(device, "last_seen_at")
                );
            }
        }
        Command::Token {
            command: crate::TokenCommand::Create { .. },
        } => println!("{}", field(value, "token")?),
        Command::DevInit { .. } => {
            for device in value["devices"]
                .as_array()
                .ok_or(Error::Local("Invalid dev-init result"))?
            {
                println!(
                    "enrolled {} ({})",
                    text(device, "label"),
                    text(device, "device_id")
                );
            }
        }
        Command::Device {
            command: crate::DeviceCommand::Revoke { .. },
        }
        | Command::Token {
            command: crate::TokenCommand::Revoke { .. },
        } => println!("revoked at {}", field(value, "revoked_at")?),
        _ => crate::domain::table(value),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn comment_control_characters_are_visible_escapes() {
        assert_eq!(
            super::escaped_body(&serde_json::json!({"body":"Öl\n\t\u{1b}"})),
            "Öl\\n\\t\\u{1b}"
        );
    }
}
