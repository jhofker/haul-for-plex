//! Generated cover art: gradient posters with the title on them, as SVG.

fn hash(s: &str) -> u64 {
    // FNV-1a: stable across runs, so the same title always gets the same colours.
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Break `text` into lines of at most `width` characters, on word boundaries.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// A 2:3 poster, or a 16:9 still when `wide`. `caption` is the small line
/// under the title (a year, or an episode code).
pub fn cover(title: &str, caption: &str, wide: bool) -> String {
    let h = hash(title);
    let hue = (h % 360) as u32;
    let hue2 = (hue + 40 + (h >> 9) as u32 % 60) % 360;
    let (w, ht) = if wide { (400, 225) } else { (300, 450) };
    let width_chars = if wide { 22 } else { 13 };
    let size = if wide { 26 } else { 36 };
    let lines = wrap(title, width_chars);
    let total = lines.len() as i32;
    let first = ht / 2 - (total - 1) * size * 6 / 10;
    let mut text = String::new();
    for (i, line) in lines.iter().enumerate() {
        text.push_str(&format!(
            r##"<text x="{}" y="{}" text-anchor="middle" font-family="Georgia, serif" font-weight="bold" font-size="{size}" fill="#fff">{}</text>"##,
            w / 2,
            first + i as i32 * size * 12 / 10,
            escape(line)
        ));
    }
    // A few soft circles so posters don't all look the same.
    let mut shapes = String::new();
    for k in 0..3u64 {
        let v = h >> (k * 8);
        shapes.push_str(&format!(
            r##"<circle cx="{}" cy="{}" r="{}" fill="#fff" opacity="0.08"/>"##,
            v % u64::from(w as u32),
            (v >> 4) % u64::from(ht as u32),
            40 + (v >> 12) % 90
        ));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {ht}" width="{w}" height="{ht}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl({hue},55%,38%)"/><stop offset="1" stop-color="hsl({hue2},60%,18%)"/></linearGradient></defs><rect width="{w}" height="{ht}" fill="url(#g)"/>{shapes}{text}<text x="{}" y="{}" text-anchor="middle" font-family="Helvetica, Arial, sans-serif" font-size="16" fill="#fff" opacity="0.8">{}</text></svg>"##,
        w / 2,
        ht - 22,
        escape(caption)
    )
}
