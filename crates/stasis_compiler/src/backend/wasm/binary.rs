pub(super) const I32: u8 = 0x7f;
pub(super) const F32: u8 = 0x7d;
pub(super) const F64: u8 = 0x7c;

pub(super) fn append_name_section(function_names: &[(u32, String)], module: &mut Vec<u8>) {
    let mut function_subsection = Vec::new();
    uleb(function_names.len() as u32, &mut function_subsection);
    for (index, name) in function_names {
        uleb(*index, &mut function_subsection);
        string(name, &mut function_subsection);
    }
    let mut payload = Vec::new();
    string("name", &mut payload);
    payload.push(1);
    uleb(function_subsection.len() as u32, &mut payload);
    payload.extend(function_subsection);
    section(0, payload, module);
}

pub(super) fn section(id: u8, payload: Vec<u8>, module: &mut Vec<u8>) {
    if payload.is_empty() {
        return;
    }
    module.push(id);
    uleb(payload.len() as u32, module);
    module.extend(payload);
}

pub(super) fn string(value: &str, out: &mut Vec<u8>) {
    uleb(value.len() as u32, out);
    out.extend(value.as_bytes());
}

pub(super) fn uleb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub(super) fn sleb(mut value: i32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            break;
        }
    }
}

pub(super) fn sleb64(mut value: i64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(encode: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut out = Vec::new();
        encode(&mut out);
        out
    }

    #[test]
    fn encodes_unsigned_leb_boundaries() {
        assert_eq!(encoded(|out| uleb(0, out)), [0]);
        assert_eq!(encoded(|out| uleb(0x7f, out)), [0x7f]);
        assert_eq!(encoded(|out| uleb(0x80, out)), [0x80, 0x01]);
        assert_eq!(
            encoded(|out| uleb(u32::MAX, out)),
            [0xff, 0xff, 0xff, 0xff, 0x0f]
        );
    }

    #[test]
    fn encodes_signed_leb_boundaries() {
        assert_eq!(encoded(|out| sleb(0, out)), [0]);
        assert_eq!(encoded(|out| sleb(63, out)), [0x3f]);
        assert_eq!(encoded(|out| sleb(64, out)), [0xc0, 0x00]);
        assert_eq!(encoded(|out| sleb(-64, out)), [0x40]);
        assert_eq!(encoded(|out| sleb(-65, out)), [0xbf, 0x7f]);
        assert_eq!(
            encoded(|out| sleb(i32::MIN, out)),
            [0x80, 0x80, 0x80, 0x80, 0x78]
        );
        assert_eq!(
            encoded(|out| sleb(i32::MAX, out)),
            [0xff, 0xff, 0xff, 0xff, 0x07]
        );
    }

    #[test]
    fn encodes_signed_64_bit_boundaries() {
        assert_eq!(
            encoded(|out| sleb64(i64::MIN, out)),
            [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7f]
        );
        assert_eq!(
            encoded(|out| sleb64(i64::MAX, out)),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]
        );
    }

    #[test]
    fn omits_empty_sections_and_encodes_payload_length() {
        let mut module = Vec::new();
        section(7, Vec::new(), &mut module);
        assert!(module.is_empty());
        section(7, vec![1, 2, 3], &mut module);
        assert_eq!(module, [7, 3, 1, 2, 3]);
    }
}
