//! Just enough DER to read what a Sigstore certificate says about its publisher and to take the public key out of a
//! key or certificate: definite lengths in their shortest form and single-byte tags. Anything else is refused. The
//! certificate chain itself is checked by webpki; this only reads fields of a certificate that already passed it.

pub const SEQUENCE: u8 = 0x30;
pub const OCTET_STRING: u8 = 0x04;
pub const BIT_STRING: u8 = 0x03;
pub const OID: u8 = 0x06;
pub const BOOLEAN: u8 = 0x01;
pub const UTF8_STRING: u8 = 0x0c;
/// `[3]` around the extensions of a certificate.
pub const EXTENSIONS: u8 = 0xa3;
/// `GeneralName` choices in a subject alternative name.
pub const SAN_EMAIL: u8 = 0x81;
pub const SAN_URI: u8 = 0x86;

pub const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
pub const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
pub const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
pub const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];

/// One element: its tag, its contents, and the whole encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    pub raw: &'a [u8],
}

/// The first element of `input` and what follows it.
pub fn read(input: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let (&tag, rest) = input.split_first()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (first as usize, rest)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n || rest[0] == 0 {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |acc, &b| (acc << 8) | b as usize);
        if len < 0x80 {
            return None;
        }
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    let header = input.len() - rest.len();
    Some((Tlv { tag, value: &rest[..len], raw: &input[..header + len] }, &rest[len..]))
}

/// The element of `input` with this tag, with nothing after it.
pub fn only(input: &[u8], tag: u8) -> Option<Tlv<'_>> {
    let (t, rest) = read(input)?;
    (t.tag == tag && rest.is_empty()).then_some(t)
}

/// The elements inside a constructed value.
pub fn children(mut value: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !value.is_empty() {
        let (t, rest) = read(value)?;
        out.push(t);
        value = rest;
    }
    Some(out)
}

/// A certificate's extensions as (OID, value) pairs; empty when it has none.
pub fn extensions(cert: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    let cert = only(cert, SEQUENCE)?;
    let tbs = *children(cert.value)?.first().filter(|t| t.tag == SEQUENCE)?;
    let Some(exts) = children(tbs.value)?.into_iter().find(|t| t.tag == EXTENSIONS) else { return Some(Vec::new()) };
    let mut out = Vec::new();
    for ext in children(only(exts.value, SEQUENCE)?.value)? {
        let parts = children(ext.value)?;
        let (oid, value) = match parts.as_slice() {
            [oid, value] => (oid, value),
            [oid, critical, value] if critical.tag == BOOLEAN => (oid, value),
            _ => return None,
        };
        if oid.tag != OID || value.tag != OCTET_STRING {
            return None;
        }
        out.push((oid.value, value.value));
    }
    Some(out)
}

/// The subject public key info of a certificate.
pub fn spki_of_cert(cert: &[u8]) -> Option<&[u8]> {
    let cert = only(cert, SEQUENCE)?;
    let tbs = *children(cert.value)?.first().filter(|t| t.tag == SEQUENCE)?;
    let fields = children(tbs.value)?;
    // version [0] is optional; then serial, signature, issuer, validity, subject, spki.
    let start = usize::from(fields.first()?.tag == 0xa0);
    let spki = fields.get(start + 5).filter(|t| t.tag == SEQUENCE)?;
    Some(spki.raw)
}

/// An EC public key: the curve's OID and the uncompressed point.
pub fn ec_point(spki: &[u8]) -> Option<(&[u8], &[u8])> {
    let parts = children(only(spki, SEQUENCE)?.value)?;
    let [alg, key] = parts.as_slice() else { return None };
    let alg = children(alg.value)?;
    let [kind, curve] = alg.as_slice() else { return None };
    if kind.tag != OID || kind.value != OID_EC_PUBLIC_KEY || curve.tag != OID || key.tag != BIT_STRING {
        return None;
    }
    let (&unused, point) = key.value.split_first()?;
    (unused == 0).then_some((curve.value, point))
}
