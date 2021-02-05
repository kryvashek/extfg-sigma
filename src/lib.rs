use std::collections::BTreeMap;

use bytes::Bytes;
use bytes::{BufMut, BytesMut};
use serde::Serialize;
use serde_json::Value;

use crate::util::*;

#[macro_use]
mod util;

// TODO: validate mandatory fields

#[derive(Debug, thiserror::Error, PartialEq, Clone)]
pub enum Error {
    #[error("{0}")]
    Bounds(String),
    #[error("Incorrect tag: {0}")]
    IncorrectTag(String),
    #[error("Incorrect field '{field_name}', should be {should_be}")]
    IncorrectFieldData {
        field_name: String,
        should_be: String,
    },
    #[error("Missing field '{0}'")]
    MissingField(String),
    #[error("{0}")]
    IncorrectData(String),
}

impl Error {
    fn incorrect_field_data(field_name: &str, should_be: &str) -> Self {
        Self::IncorrectFieldData {
            field_name: field_name.into(),
            should_be: should_be.into(),
        }
    }
}

#[derive(Serialize, Debug)]
pub struct SigmaRequest {
    pub saf: String,
    pub source: String,
    pub mti: String,
    pub auth_serno: u64,
    pub tags: BTreeMap<u16, String>,
    pub iso_fields: BTreeMap<u16, String>,
    pub iso_subfields: BTreeMap<(u16, u8), String>,
}

impl SigmaRequest {
    pub fn new(saf: &str, source: &str, mti: &str, auth_serno: u64) -> Self {
        Self {
            saf: saf.into(),
            source: source.into(),
            mti: mti.into(),
            auth_serno,
            tags: Default::default(),
            iso_fields: Default::default(),
            iso_subfields: Default::default(),
        }
    }

    pub fn from_json_value(mut data: Value) -> Result<SigmaRequest, Error> {
        let data = data.as_object_mut().ok_or(Error::IncorrectData(
            "SigmaRequest JSON should be object".into(),
        ))?;
        let mut req = Self::new("N", "X", "0100", 0);

        macro_rules! fill_req_field {
            ($fname:ident, $pname:literal, $comment:literal) => {
                match data.remove($pname) {
                    Some(x) => match x.as_str() {
                        Some(v) => {
                            req.$fname = v.to_string();
                        }
                        None => {
                            return Err(Error::IncorrectFieldData {
                                field_name: $pname.to_string(),
                                should_be: $comment.to_string(),
                            });
                        }
                    },
                    None => {
                        return Err(Error::MissingField($pname.to_string()));
                    }
                }
            };
        }

        fill_req_field!(saf, "SAF", "String");
        fill_req_field!(source, "SRC", "String");
        fill_req_field!(mti, "MTI", "String");
        // Authorization serno
        match data.remove("Serno") {
            Some(x) => {
                if let Some(s) = x.as_str() {
                    req.auth_serno = s.parse::<u64>().map_err(|_| Error::IncorrectFieldData {
                        field_name: "Serno".into(),
                        should_be: "integer".into(),
                    })?;
                } else if let Some(v) = x.as_u64() {
                    req.auth_serno = v;
                } else {
                    return Err(Error::IncorrectFieldData {
                        field_name: "Serno".into(),
                        should_be: "u64 or String with integer".into(),
                    });
                }
            }
            None => {
                req.auth_serno = util::gen_random_auth_serno();
            }
        }

        for (name, field_data) in data.iter() {
            let tag = Tag::from_str(&name)?;
            let content = if let Some(x) = field_data.as_str() {
                x.into()
            } else if let Some(x) = field_data.as_u64() {
                format!("{}", x)
            } else {
                return Err(Error::IncorrectFieldData {
                    field_name: name.clone(),
                    should_be: "u64 or String with integer".into(),
                });
            };
            match tag {
                Tag::Regular(i) => req.tags.insert(i, content),
                Tag::Iso(i) => req.iso_fields.insert(i, content),
                Tag::IsoSubfield(i, si) => req.iso_subfields.insert((i, si), content),
            };
        }

        Ok(req)
    }

    // TODO: access to fields

    pub fn encode(&self) -> Result<Bytes, Error> {
        let mut buf = BytesMut::with_capacity(8192);
        buf.put(self.saf.as_bytes());
        buf.put(self.source.as_bytes());
        buf.put(self.mti.as_bytes());
        if self.auth_serno > 9999999999 {
            buf.put(&format!("{}", self.auth_serno).as_bytes()[0..10]);
        } else {
            buf.put(format!("{:010}", self.auth_serno).as_bytes());
        }

        for (k, v) in self.tags.iter() {
            encode_field_to_buf(Tag::Regular(*k), &v, &mut buf)?;
        }

        for (k, v) in self.iso_fields.iter() {
            encode_field_to_buf(Tag::Iso(*k), &v, &mut buf)?;
        }

        for ((k, k1), v) in self.iso_subfields.iter() {
            encode_field_to_buf(Tag::IsoSubfield(*k, *k1), &v, &mut buf)?;
        }

        let mut buf_res = BytesMut::with_capacity(buf.len() + 10);
        buf_res.put(format!("{:05}", buf.len()).as_bytes());
        buf_res.put(buf);

        Ok(buf_res.into())
    }
}

#[derive(Serialize, Debug)]
pub struct FeeData {
    pub reason: u16,
    pub currency: u16,
    pub amount: u64,
}

impl FeeData {
    pub fn from_slice(data: &[u8]) -> Result<Self, Error> {
        if data.len() >= 8 {
            // "\x00\x32\x00\x00\x108116978300"
            let reason = parse_ascii_bytes!(
                &data[0..4],
                u16,
                Error::incorrect_field_data("FeeData.reason", "valid integer")
            )?;
            let currency = parse_ascii_bytes!(
                &data[4..7],
                u16,
                Error::incorrect_field_data("FeeData.currency", "valid integer")
            )?;
            let amount = parse_ascii_bytes!(
                &data[7..],
                u64,
                Error::incorrect_field_data("FeeData.amount", "valid integer")
            )?;
            Ok(Self {
                reason,
                currency,
                amount,
            })
        } else {
            Err(Error::IncorrectData(
                "FeeData slice should be longer than 8 bytes".into(),
            ))
        }
    }
}

#[derive(Serialize, Debug)]
pub struct SigmaResponse {
    pub mti: String,
    pub auth_serno: u64,
    pub reason: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fees: Vec<FeeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adata: Option<String>,
}

fn bytes_split_to(bytes: &mut Bytes, at: usize) -> Result<Bytes, Error> {
    let len = bytes.len();

    if len < at {
        return Err(Error::Bounds(format!(
            "split_to out of bounds: {:?} <= {:?}",
            at,
            bytes.len(),
        )));
    }

    Ok(bytes.split_to(at))
}

impl SigmaResponse {
    pub fn new(mti: &str, auth_serno: u64, reason: u32) -> Self {
        Self {
            mti: mti.into(),
            auth_serno,
            reason,
            fees: Vec::new(),
            adata: Option::None,
        }
    }

    pub fn decode(mut data: Bytes) -> Result<Self, Error> {
        let mut resp = Self::new("0100", 0, 0);

        let msg_len = parse_ascii_bytes!(
            &bytes_split_to(&mut data, 5)?,
            usize,
            Error::incorrect_field_data("message length", "valid integer")
        )?;
        let mut data = bytes_split_to(&mut data, msg_len)?;

        resp.mti = String::from_utf8_lossy(&bytes_split_to(&mut data, 4)?).to_string();
        resp.auth_serno = String::from_utf8_lossy(&bytes_split_to(&mut data, 10)?)
            .trim()
            .parse::<u64>()
            .map_err(|_| Error::IncorrectFieldData {
                field_name: "Serno".into(),
                should_be: "u64".into(),
            })?;

        while !data.is_empty() {
            /*
             *  |
             *  |  T  | \x00 | \x31 | \x00 | \x00 | \x04 |  8  |  1  |  0  |  0  |
             *        |             |      |             |                       |
             *        |__ tag id ___|      |tag data len |_______ data __________|
             */
            let tag_src = bytes_split_to(&mut data, 4)?;
            let tag = Tag::decode(tag_src)?;

            let len_src = bytes_split_to(&mut data, 2)?;
            let len = decode_bcd_x4(&[len_src[0], len_src[1]])?;

            let data_src = bytes_split_to(&mut data, len as usize)?;

            match tag {
                Tag::Regular(31) => {
                    resp.reason = parse_ascii_bytes!(
                        &data_src,
                        u32,
                        Error::incorrect_field_data("reason", "shloud be u32")
                    )?;
                }
                Tag::Regular(32) => {
                    resp.fees.push(FeeData::from_slice(&data_src)?);
                }
                Tag::Regular(48) => {
                    resp.adata = Some(String::from_utf8_lossy(&data_src).to_string());
                }
                _ => {}
            }
        }

        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok() {
        let payload = r#"{
            "SAF": "Y",
            "SRC": "M",
            "MTI": "0200",
            "Serno": 6007040979,
            "T0000": 2371492071643,
            "T0001": "C",
            "T0002": 643,
            "T0003": "000100000000",
            "T0004": 978,
            "T0005": "000300000000",
            "T0006": "OPS6",
            "T0007": 19,
            "T0008": 643,
            "T0009": 3102,
            "T0010": 3104,
            "T0011": 2,
            "T0014": "IDDQD Bank",
            "T0016": 74707182,
            "T0018": "Y",
            "T0022": "000000000010",
            "i000": "0100",
            "i002": "555544******1111",
            "i003": "500000",
            "i004": "000100000000",
            "i006": "000100000000",
            "i007": "0629151748",
            "i011": "100250",
            "i012": "181748",
            "i013": "0629",
            "i018": "0000",
            "i022": "0000",
            "i025": "02",
            "i032": "010455",
            "i037": "002595100250",
            "i041": 990,
            "i042": "DCZ1",
            "i043": "IDDQD Bank.                         GE",
            "i048": "USRDT|2595100250",
            "i049": 643,
            "i051": 643,
            "i060": 3,
            "i101": 91926242,
            "i102": 2371492071643
        }"#;

        let r: SigmaRequest =
            SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).unwrap();
        assert_eq!(r.saf, "Y");
        assert_eq!(r.source, "M");
        assert_eq!(r.mti, "0200");
        assert_eq!(r.auth_serno, 6007040979);
        assert_eq!(r.tags.get(&0).unwrap(), "2371492071643");
        assert_eq!(r.tags.get(&1).unwrap(), "C");
        assert_eq!(r.tags.get(&2).unwrap(), "643");
        assert_eq!(r.tags.get(&3).unwrap(), "000100000000");
        assert_eq!(r.tags.get(&4).unwrap(), "978");
        assert_eq!(r.tags.get(&5).unwrap(), "000300000000");
        assert_eq!(r.tags.get(&6).unwrap(), "OPS6");
        assert_eq!(r.tags.get(&7).unwrap(), "19");
        assert_eq!(r.tags.get(&8).unwrap(), "643");
        assert_eq!(r.tags.get(&9).unwrap(), "3102");
        assert_eq!(r.tags.get(&10).unwrap(), "3104");
        assert_eq!(r.tags.get(&11).unwrap(), "2");

        if r.tags.get(&12).is_some() {
            unreachable!();
        }

        if r.tags.get(&13).is_some() {
            unreachable!();
        }

        assert_eq!(r.tags.get(&14).unwrap(), "IDDQD Bank");

        if r.tags.get(&15).is_some() {
            unreachable!();
        }

        assert_eq!(r.tags.get(&16).unwrap(), "74707182");
        if r.tags.get(&17).is_some() {
            unreachable!();
        }
        assert_eq!(r.tags.get(&18).unwrap(), "Y");
        assert_eq!(r.tags.get(&22).unwrap(), "000000000010");

        assert_eq!(r.iso_fields.get(&0).unwrap(), "0100");

        if r.iso_fields.get(&1).is_some() {
            unreachable!();
        }

        assert_eq!(r.iso_fields.get(&2).unwrap(), "555544******1111");
        assert_eq!(r.iso_fields.get(&3).unwrap(), "500000");
        assert_eq!(r.iso_fields.get(&4).unwrap(), "000100000000");
        assert_eq!(r.iso_fields.get(&6).unwrap(), "000100000000");
        assert_eq!(r.iso_fields.get(&7).unwrap(), "0629151748");
        assert_eq!(r.iso_fields.get(&11).unwrap(), "100250");
        assert_eq!(r.iso_fields.get(&12).unwrap(), "181748");
        assert_eq!(r.iso_fields.get(&13).unwrap(), "0629");
        assert_eq!(r.iso_fields.get(&18).unwrap(), "0000");
        assert_eq!(r.iso_fields.get(&22).unwrap(), "0000");
        assert_eq!(r.iso_fields.get(&25).unwrap(), "02");
        assert_eq!(r.iso_fields.get(&32).unwrap(), "010455");
        assert_eq!(r.iso_fields.get(&37).unwrap(), "002595100250");
        assert_eq!(r.iso_fields.get(&41).unwrap(), "990");
        assert_eq!(r.iso_fields.get(&42).unwrap(), "DCZ1");
        assert_eq!(
            r.iso_fields.get(&43).unwrap(),
            "IDDQD Bank.                         GE"
        );
        assert_eq!(r.iso_fields.get(&48).unwrap(), "USRDT|2595100250");
        assert_eq!(r.iso_fields.get(&49).unwrap(), "643");
        assert_eq!(r.iso_fields.get(&51).unwrap(), "643");
        assert_eq!(r.iso_fields.get(&60).unwrap(), "3");
        assert_eq!(r.iso_fields.get(&101).unwrap(), "91926242");
        assert_eq!(r.iso_fields.get(&102).unwrap(), "2371492071643");
    }

    #[test]
    fn serno_as_string() {
        let payload = r#"{
            "SAF": "Y",
            "SRC": "M",
            "MTI": "0200",
            "Serno": "0600704097",
            "T0000": 2371492071643,
            "T0001": "C",
            "T0002": 643,
            "T0003": "000100000000",
            "T0004": 978,
            "T0005": "000300000000",
            "T0006": "OPS6",
            "T0007": 19,
            "T0008": 643,
            "T0009": 3102,
            "T0010": 3104,
            "T0011": 2,
            "T0014": "IDDQD Bank",
            "T0016": 74707182,
            "T0018": "Y",
            "T0022": "000000000010",
            "i000": "0100",
            "i002": "555544******1111",
            "i003": "500000",
            "i004": "000100000000",
            "i006": "000100000000",
            "i007": "0629151748",
            "i011": "100250",
            "i012": "181748",
            "i013": "0629",
            "i018": "0000",
            "i022": "0000",
            "i025": "02",
            "i032": "010455",
            "i037": "002595100250",
            "i041": 990,
            "i042": "DCZ1",
            "i043": "IDDQD Bank.                         GE",
            "i048": "USRDT|2595100250",
            "i049": 643,
            "i051": 643,
            "i060": 3,
            "i101": 91926242,
            "i102": 2371492071643
        }"#;

        let r: SigmaRequest =
            SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).unwrap();
        assert_eq!(r.saf, "Y");
        assert_eq!(r.source, "M");
        assert_eq!(r.mti, "0200");
        assert_eq!(r.auth_serno, 600704097);
        assert_eq!(r.tags.get(&0).unwrap(), "2371492071643");
        assert_eq!(r.tags.get(&1).unwrap(), "C");
        assert_eq!(r.tags.get(&2).unwrap(), "643");
        assert_eq!(r.tags.get(&3).unwrap(), "000100000000");
        assert_eq!(r.tags.get(&4).unwrap(), "978");
        assert_eq!(r.tags.get(&5).unwrap(), "000300000000");
        assert_eq!(r.tags.get(&6).unwrap(), "OPS6");
        assert_eq!(r.tags.get(&7).unwrap(), "19");
        assert_eq!(r.tags.get(&8).unwrap(), "643");
        assert_eq!(r.tags.get(&9).unwrap(), "3102");
        assert_eq!(r.tags.get(&10).unwrap(), "3104");
        assert_eq!(r.tags.get(&11).unwrap(), "2");

        if r.tags.get(&12).is_some() {
            unreachable!();
        }

        if r.tags.get(&13).is_some() {
            unreachable!();
        }

        assert_eq!(r.tags.get(&14).unwrap(), "IDDQD Bank");

        if r.tags.get(&15).is_some() {
            unreachable!();
        }

        assert_eq!(r.tags.get(&16).unwrap(), "74707182");
        if r.tags.get(&17).is_some() {
            unreachable!();
        }
        assert_eq!(r.tags.get(&18).unwrap(), "Y");
        assert_eq!(r.tags.get(&22).unwrap(), "000000000010");

        assert_eq!(r.iso_fields.get(&0).unwrap(), "0100");

        if r.iso_fields.get(&1).is_some() {
            unreachable!();
        }

        assert_eq!(r.iso_fields.get(&2).unwrap(), "555544******1111");
        assert_eq!(r.iso_fields.get(&3).unwrap(), "500000");
        assert_eq!(r.iso_fields.get(&4).unwrap(), "000100000000");
        assert_eq!(r.iso_fields.get(&6).unwrap(), "000100000000");
        assert_eq!(r.iso_fields.get(&7).unwrap(), "0629151748");
        assert_eq!(r.iso_fields.get(&11).unwrap(), "100250");
        assert_eq!(r.iso_fields.get(&12).unwrap(), "181748");
        assert_eq!(r.iso_fields.get(&13).unwrap(), "0629");
        assert_eq!(r.iso_fields.get(&18).unwrap(), "0000");
        assert_eq!(r.iso_fields.get(&22).unwrap(), "0000");
        assert_eq!(r.iso_fields.get(&25).unwrap(), "02");
        assert_eq!(r.iso_fields.get(&32).unwrap(), "010455");
        assert_eq!(r.iso_fields.get(&37).unwrap(), "002595100250");
        assert_eq!(r.iso_fields.get(&41).unwrap(), "990");
        assert_eq!(r.iso_fields.get(&42).unwrap(), "DCZ1");
        assert_eq!(
            r.iso_fields.get(&43).unwrap(),
            "IDDQD Bank.                         GE"
        );
        assert_eq!(r.iso_fields.get(&48).unwrap(), "USRDT|2595100250");
        assert_eq!(r.iso_fields.get(&49).unwrap(), "643");
        assert_eq!(r.iso_fields.get(&51).unwrap(), "643");
        assert_eq!(r.iso_fields.get(&60).unwrap(), "3");
        assert_eq!(r.iso_fields.get(&101).unwrap(), "91926242");
        assert_eq!(r.iso_fields.get(&102).unwrap(), "2371492071643");
    }

    #[test]
    fn missing_saf() {
        let payload = r#"{
            "SRC": "M",
            "MTI": "0200"
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if mandatory field is missing");
        }
    }

    #[test]
    fn invalid_saf() {
        let payload = r#"{
        	"SAF": 1234,
            "SRC": "M",
            "MTI": "0200"
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if the filed has invalid format");
        }
    }

    #[test]
    fn missing_source() {
        let payload = r#"{
        	"SAF": "N",
            "MTI": "0200"
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if mandatory field is missing");
        }
    }

    #[test]
    fn invalid_source() {
        let payload = r#"{
        	"SAF": "N",
            "SRC": 929292,
            "MTI": "0200"
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if the filed has invalid format");
        }
    }

    #[test]
    fn missing_mti() {
        let payload = r#"{
        	"SAF": "N",
        	"SRC": "O"
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if mandatory field is missing");
        }
    }

    #[test]
    fn invalid_mti() {
        let payload = r#"{
        	"SAF": "N",
            "SRC": "O",
            "MTI": 1200
        }"#;

        if SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).is_ok() {
            unreachable!("Should not return Ok if the filed has invalid format");
        }
    }

    #[test]
    fn generating_auth_serno() {
        let payload = r#"{
                "SAF": "Y",
                "SRC": "M",
                "MTI": "0200",
                "T0000": "02371492071643"
            }"#;

        let r: SigmaRequest =
            SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).unwrap();
        assert!(
            r.auth_serno > 0,
            "Should generate authorization serno if the field is missing"
        );
    }

    #[test]
    fn serializing_generated_auth_serno() {
        let payload = r#"{
                "SAF": "Y",
                "SRC": "M",
                "MTI": "0201",
                "Serno": 7877706965687192023
            }"#;

        let r: SigmaRequest =
            SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).unwrap();
        let serialized = r.encode().unwrap();
        assert_eq!(
            serialized,
            b"00016YM02017877706965"[..],
            "Original auth serno should be trimmed to 10 bytes"
        );
    }

    #[test]
    fn serializing_ok() {
        let payload = r#"{
                "SAF": "Y",
                "SRC": "M",
                "MTI": "0200",
                "Serno": 6007040979,
                "T0000": 2371492071643,
                "T0001": "C",
                "T0002": 643,
                "T0003": "000100000000",
                "T0004": 978,
                "T0005": "000300000000",
                "T0006": "OPS6",
                "T0007": 19,
                "T0008": 643,
                "T0009": 3102,
                "T0010": 3104,
                "T0011": 2,
                "T0014": "IDDQD Bank",
                "T0016": 74707182,
                "T0018": "Y",
                "T0022": "000000000010",
                "i000": "0100",
                "i002": "555544******1111",
                "i003": "500000",
                "i004": "000100000000",
                "i006": "000100000000",
                "i007": "0629151748",
                "i011": "100250",
                "i012": "181748",
                "i013": "0629",
                "i018": "0000",
                "i022": "0000",
                "i025": "02",
                "i032": "010455",
                "i037": "002595100250",
                "i041": 990,
                "i042": "DCZ1",
                "i043": "IDDQD Bank.                         GE",
                "i048": "USRDT|2595100250",
                "i049": 643,
                "i051": 643,
                "i060": 3,
                "i101": 91926242,
                "i102": 2371492071643
            }"#;

        let r: SigmaRequest =
            SigmaRequest::from_json_value(serde_json::from_str(&payload).unwrap()).unwrap();
        let serialized = r.encode().unwrap();
        assert_eq!(
            serialized,
            b"00536YM02006007040979T\x00\x00\x00\x00\x132371492071643T\x00\x01\x00\x00\x01CT\x00\x02\x00\x00\x03643T\x00\x03\x00\x00\x12000100000000T\x00\x04\x00\x00\x03978T\x00\x05\x00\x00\x12000300000000T\x00\x06\x00\x00\x04OPS6T\x00\x07\x00\x00\x0219T\x00\x08\x00\x00\x03643T\x00\t\x00\x00\x043102T\x00\x10\x00\x00\x043104T\x00\x11\x00\x00\x012T\x00\x14\x00\x00\x10IDDQD BankT\x00\x16\x00\x00\x0874707182T\x00\x18\x00\x00\x01YT\x00\x22\x00\x00\x12000000000010I\x00\x00\x00\x00\x040100I\x00\x02\x00\x00\x16555544******1111I\x00\x03\x00\x00\x06500000I\x00\x04\x00\x00\x12000100000000I\x00\x06\x00\x00\x12000100000000I\x00\x07\x00\x00\x100629151748I\x00\x11\x00\x00\x06100250I\x00\x12\x00\x00\x06181748I\x00\x13\x00\x00\x040629I\x00\x18\x00\x00\x040000I\x00\"\x00\x00\x040000I\x00%\x00\x00\x0202I\x002\x00\x00\x06010455I\x007\x00\x00\x12002595100250I\x00A\x00\x00\x03990I\x00B\x00\x00\x04DCZ1I\x00C\x00\x008IDDQD Bank.                         GEI\x00H\x00\x00\x16USRDT|2595100250I\x00I\x00\x00\x03643I\x00Q\x00\x00\x03643I\x00`\x00\x00\x013I\x01\x01\x00\x00\x0891926242I\x01\x02\x00\x00\x132371492071643"[..]
        );
    }

    #[test]
    fn sigma_response_decode() {
        let s = Bytes::from_static(b"0002401104007040978T\x00\x31\x00\x00\x048495");

        let resp = SigmaResponse::decode(s).unwrap();
        assert_eq!(resp.mti, "0110");
        assert_eq!(resp.auth_serno, 4007040978);
        assert_eq!(resp.reason, 8495);

        let serialized = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            serialized,
            r#"{"mti":"0110","auth_serno":4007040978,"reason":8495}"#
        );
    }

    #[test]
    fn sigma_response_incorrect_auth_serno() {
        let s = Bytes::from_static(b"000250110XYZ7040978T\x00\x31\x00\x00\x048100");

        assert!(SigmaResponse::decode(s).is_err());
    }

    #[test]
    fn sigma_response_incorrect_reason() {
        let s = Bytes::from_static(b"0002501104007040978T\x00\x31\x00\x00\x04ABCD");

        assert!(SigmaResponse::decode(s).is_err());
    }

    #[test]
    fn sigma_response_fee_data() {
        let s = Bytes::from_static(
            b"0004001104007040978T\x00\x31\x00\x00\x048100T\x00\x32\x00\x00\x108116978300",
        );

        let resp = SigmaResponse::decode(s).unwrap();
        assert_eq!(resp.mti, "0110");
        assert_eq!(resp.auth_serno, 4007040978);
        assert_eq!(resp.reason, 8100);

        let serialized = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            serialized,
            r#"{"mti":"0110","auth_serno":4007040978,"reason":8100,"fees":[{"reason":8116,"currency":978,"amount":300}]}"#
        );
    }

    #[test]
    fn sigma_response_correct_short_auth_serno() {
        let s = Bytes::from_static(b"000240110123123    T\x00\x31\x00\x00\x048100");

        let resp = SigmaResponse::decode(s).unwrap();
        assert_eq!(resp.mti, "0110");
        assert_eq!(resp.auth_serno, 123123);
        assert_eq!(resp.reason, 8100);

        let serialized = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            serialized,
            r#"{"mti":"0110","auth_serno":123123,"reason":8100}"#
        );
    }

    #[test]
    fn fee_data() {
        let data = b"8116978300";

        let fee = FeeData::from_slice(data).unwrap();
        assert_eq!(fee.reason, 8116);
        assert_eq!(fee.currency, 978);
        assert_eq!(fee.amount, 300);
    }

    #[test]
    fn fee_data_large_amount() {
        let data = b"8116643123456789";

        let fee = FeeData::from_slice(data).unwrap();
        assert_eq!(fee.reason, 8116);
        assert_eq!(fee.currency, 643);
        assert_eq!(fee.amount, 123456789);
    }

    #[test]
    fn sigma_response_fee_data_additional_data() {
        let s = Bytes::from_static(b"0015201104007040978T\x00\x31\x00\x00\x048100T\x00\x32\x00\x00\x1181166439000T\x00\x48\x00\x01\x05CJyuARCDBRibpKn+BSIVCgx0ZmE6FwAAAKoXmwIQnK4BGLcBIhEKDHRmcDoWAAAAxxX+ARik\nATCBu4PdBToICKqv7BQQgwVAnK4BSAI=");

        let resp = SigmaResponse::decode(s).unwrap();
        assert_eq!(resp.mti, "0110");
        assert_eq!(resp.auth_serno, 4007040978);
        assert_eq!(resp.reason, 8100);

        let serialized = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            serialized,
            r#"{"mti":"0110","auth_serno":4007040978,"reason":8100,"fees":[{"reason":8116,"currency":643,"amount":9000}],"adata":"CJyuARCDBRibpKn+BSIVCgx0ZmE6FwAAAKoXmwIQnK4BGLcBIhEKDHRmcDoWAAAAxxX+ARik\nATCBu4PdBToICKqv7BQQgwVAnK4BSAI="}"#
        );
    }
}
