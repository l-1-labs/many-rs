use many_types::AttributeRelatedIndex;
use minicbor::{Encode, Decode};
use num_bigint::BigInt;

pub type DataIndex = AttributeRelatedIndex;

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub enum DataType {
    #[n(0)]
    Counter,
    #[n(1)]
    Gauge,
}

#[derive(Clone, Decode, Encode, PartialEq, Eq, Debug)]
pub enum DataValue {
    #[n(0)]
    Counter(#[n(0)] DataValueTypeCounter),
    #[n(1)]
    Gauge(#[n(0)] DataValueTypeGauge),
}

pub type DataValueTypeCounter = u64;

#[derive(Clone, Decode, Encode, Debug)]
pub enum DataValueTypeGauge {
    #[n(0)]
    Int(#[n(0)] i64),
    #[n(1)]
    Float(#[n(0)] f64),
    #[n(2)]
    BigInt(#[cbor(n(0), decode_with="decode_bigint", encode_with = "encode_bigint")] BigInt)
}

impl PartialEq<DataValueTypeGauge> for DataValueTypeGauge {
    fn eq(&self, other: &DataValueTypeGauge) -> bool {
        match (self, other) {
            (DataValueTypeGauge::BigInt(a),
             DataValueTypeGauge::Int(b)) => &BigInt::from(*b) == a,
            (DataValueTypeGauge::BigInt(a),
             DataValueTypeGauge::Float(b)) => b.fract() != 0.0 && &BigInt::from(*b as i64) == a,
            (DataValueTypeGauge::Int(a),
             DataValueTypeGauge::BigInt(b)) => &BigInt::from(*a) == b,
            (DataValueTypeGauge::Float(a),
             DataValueTypeGauge::BigInt(b)) => a.fract() != 0.0 && &BigInt::from(*a as i64) == b,
            (DataValueTypeGauge::Int(a), DataValueTypeGauge::Int(b)) => a == b,
            (DataValueTypeGauge::BigInt(a), DataValueTypeGauge::BigInt(b)) => a == b,
            (DataValueTypeGauge::Float(a), DataValueTypeGauge::Float(b)) => a == b,
            (DataValueTypeGauge::Int(a),
             DataValueTypeGauge::Float(b)) => b.fract() != 0.0 && (*b as i64) == *a,
            (DataValueTypeGauge::Float(a),
             DataValueTypeGauge::Int(b)) => a.fract() != 0.0 && (*a as i64) == *b,
        }
    }
}

impl Eq for DataValueTypeGauge {
}

fn decode_bigint<C>(
    d: &mut minicbor::Decoder<'_>,
    _: &mut C
) -> Result<BigInt, minicbor::decode::Error> {
    let vec: Vec<u8> = d.decode()?;
    Ok(BigInt::from_signed_bytes_be(vec.as_slice()))
}

fn encode_bigint<C, W: minicbor::encode::Write>(
    v: &BigInt,
    e: &mut minicbor::Encoder<W>,
    _: &mut C
) -> Result<(), minicbor::encode::Error<W::Error>> {
    e.encode(v.to_signed_bytes_be())?;
    Ok(())
}

#[derive(Clone, Debug, Encode, Decode, Eq, PartialEq)]
pub struct DataInfo {
    #[n(0)]
    pub r#type: DataType,
    #[n(1)]
    pub shortname: String,
}
