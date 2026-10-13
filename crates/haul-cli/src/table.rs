/// Print rows as left-aligned columns separated by two spaces.
pub fn print<const N: usize>(headers: [&str; N], rows: &[[String; N]]) {
    let mut widths = headers.map(|h| h.chars().count());
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        println!("{}", padded.join("  ").trim_end());
    };
    line(headers.to_vec());
    for row in rows {
        line(row.iter().map(String::as_str).collect());
    }
}

/// Format a byte count as e.g. `1.4 GB`.
pub fn size(bytes: u64) -> String {
    haul_core::format_size(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes() {
        assert_eq!(super::size(512), "512 B");
        assert_eq!(super::size(1_500_000), "1.5 MB");
        assert_eq!(super::size(4_200_000_000), "4.2 GB");
    }
}
