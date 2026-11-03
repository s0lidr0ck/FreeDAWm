//! What the status feed reports about this PC.

/// Free bytes on the disk holding `path` (or its nearest existing parent).
pub fn disk_free(path: &std::path::Path) -> Option<u64> {
    let mut p = Some(path);
    while let Some(dir) = p {
        if dir.exists() {
            return fs4::available_space(dir).ok();
        }
        p = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_temp_disk_has_some_room() {
        let free = super::disk_free(&std::env::temp_dir().join("no-such-dir-here")).unwrap();
        assert!(free > 0);
    }
}

/// This PC's address on the local network (the one its default route leaves from; no packet is
/// sent), for the page's URL.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

/// `text` as a QR code: its width and its modules row by row (`true`: dark).
pub fn qr_matrix(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width();
    Some((w, code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()))
}

#[cfg(test)]
mod qr_tests {
    #[test]
    fn a_url_makes_a_square_code() {
        let (w, m) = super::qr_matrix("http://192.168.1.20:48100/").unwrap();
        assert!(w >= 21);
        assert_eq!(m.len(), w * w);
        assert!(m.iter().any(|d| *d) && m.iter().any(|d| !*d));
    }
}
