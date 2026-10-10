//! Shared password acquisition for enrollment and admin PAT creation.
use crate::client::{Error, Result};
use std::io::IsTerminal;
use zeroize::Zeroizing;

pub fn password() -> Result<Zeroizing<String>> {
    match std::env::var("PORCHLIGHT_PASSWORD") {
        Ok(password) => Ok(Zeroizing::new(password)),
        Err(std::env::VarError::NotPresent) => {
            // rpassword opens /dev/tty, even if stdin is redirected. Never prompt in that case.
            if !std::io::stdin().is_terminal() {
                return Err(Error::Local(
                    "No terminal for password prompt; set PORCHLIGHT_PASSWORD",
                ));
            }
            rpassword::prompt_password("Password: ")
                .map(Zeroizing::new)
                .map_err(|_| {
                    Error::Local("No terminal for password prompt; set PORCHLIGHT_PASSWORD")
                })
        }
        Err(_) => Err(Error::Local("PORCHLIGHT_PASSWORD must be valid Unicode")),
    }
}
