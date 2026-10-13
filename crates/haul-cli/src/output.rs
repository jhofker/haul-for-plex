use serde::Serialize;

/// Print `value` as pretty JSON on stdout.
pub fn json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("error: could not encode JSON: {e}"),
    }
}
