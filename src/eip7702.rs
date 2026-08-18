//! EIP-7702 — "set code" transactions for EOA delegation (gas-fee sponsorship).
//!
//! A user delegates their EOA to a [`SponsorAccount`] contract by sending a
//! type-0x04 transaction whose authorization list names the delegate. After
//! that, calls TO the user's EOA run the delegate's code in the user's
//! context — and the sponsor can push executions while paying the gas.
//!
//! Formats (EIP-7702):
//! - tx:      `0x04 || rlp([chain_id, nonce, prio, max_fee, gas, to, value,
//!            data, access_list, authorization_list, y_parity, r, s])`
//! - auth:    `rlp([chain_id, address, nonce, y_parity, r, s])`
//! - digest:  `keccak256(0x05 || rlp([chain_id, address, nonce]))`

use anyhow::{bail, Result};
use k256::elliptic_curve::point::AffineCoordinates;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::elliptic_curve::PrimeField;
use k256::{ProjectivePoint, Scalar};
use sha3::{Digest, Keccak256};

/// One authorization tuple: delegate the signer's EOA to `address`.
#[derive(Debug, Clone)]
pub struct Authorization {
    pub chain_id: u64,
    pub address: String,
    /// The EOA's current nonce.
    pub nonce: u64,
}

/// A type-0x04 set-code transaction.
#[derive(Debug, Clone)]
pub struct Tx7702 {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee: u128,
    pub max_fee: u128,
    pub gas: u64,
    pub to: String,
    pub value: u128,
    pub data: Vec<u8>,
    /// `Some` = set delegation, `None` = empty list (clears nothing — use the
    /// zero address in the tuple to clear delegation per EIP-7702).
    pub authorization: Option<Authorization>,
}

/// Sign a 32-byte digest with secp256k1 and return (y_parity, r, s).
///
/// Implemented manually with RFC6979 (HMAC-SHA256 DRBG) so the k-derivation
/// matches viem/ethers (@noble/curves): h1 = the digest itself. k256's own
/// `sign_prehash` re-hashes the digest with SHA-256 for k, producing
/// signatures that don't match other wallets.
pub fn sign_hash(hash: &[u8; 32], secret: &[u8; 32]) -> Result<(u64, Vec<u8>, Vec<u8>)> {
    let d = Option::<Scalar>::from(Scalar::from_repr((*secret).into()))
        .ok_or_else(|| anyhow::anyhow!("invalid signing key"))?;
    let z = Option::<Scalar>::from(Scalar::from_repr((*hash).into())).unwrap_or(Scalar::ZERO);

    // Deterministic ephemeral key: RFC6979 with h1 = hash (no re-hash).
    let k = rfc6979_k(secret, hash)?;
    let kinv =
        Option::<Scalar>::from(k.invert()).ok_or_else(|| anyhow::anyhow!("k not invertible"))?;

    let r_point = ProjectivePoint::GENERATOR * k;
    let aff = r_point.to_affine();
    let r = Option::<Scalar>::from(Scalar::from_repr(aff.x())).unwrap_or(Scalar::ZERO);
    if r.is_zero().into() {
        bail!("r is zero — retry with a different nonce");
    }
    let zr = z + (r * d);
    let mut s = kinv * zr;
    // Normalize s (EIP-2 low-s). Normalizing flips the signature point to
    // -R, so the y-parity must flip along with it.
    let half_n_bytes: [u8; 32] =
        hex::decode("7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("bad n/2"))?;
    let half_n = Option::<Scalar>::from(Scalar::from_repr(half_n_bytes.into())).unwrap();
    let flipped = s > half_n;
    if flipped {
        s = Scalar::ZERO - s;
    }

    // y_parity = parity of R.y (recovery id bit 0), flipped with s.
    let enc = aff.to_encoded_point(false);
    let bytes = enc.as_bytes();
    let y_parity = ((bytes[64] & 1) as u64) ^ (flipped as u64);

    Ok((y_parity, r.to_bytes().to_vec(), s.to_bytes().to_vec()))
}

/// RFC6979 (HMAC-SHA256 DRBG) ephemeral scalar for secp256k1, with the
/// digest used directly as h1 (matching @noble/curves).
fn rfc6979_k(secret: &[u8; 32], z: &[u8; 32]) -> Result<Scalar> {
    use hmac::{Hmac, Mac};
    use k256::Scalar;
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut k = [0u8; 32];
    let mut v = [1u8; 32];
    let mac = |key: &[u8], data: &[u8]| -> [u8; 32] {
        let mut m = HmacSha256::new_from_slice(key).expect("hmac key");
        m.update(data);
        m.finalize().into_bytes().into()
    };

    // K = HMAC(K, V || 0x00 || x || h1)
    let mut buf = Vec::with_capacity(32 + 1 + 32 + 32);
    buf.extend_from_slice(&v);
    buf.push(0x00);
    buf.extend_from_slice(secret);
    buf.extend_from_slice(z);
    k = mac(&k, &buf);
    // V = HMAC(K, V)
    v = mac(&k, &v);
    // K = HMAC(K, V || 0x01 || x || h1)
    let mut buf = Vec::with_capacity(32 + 1 + 32 + 32);
    buf.extend_from_slice(&v);
    buf.push(0x01);
    buf.extend_from_slice(secret);
    buf.extend_from_slice(z);
    k = mac(&k, &buf);
    // V = HMAC(K, V)
    v = mac(&k, &v);

    loop {
        // V = HMAC(K, V)
        v = mac(&k, &v);
        if let Some(scalar) = Option::<Scalar>::from(Scalar::from_repr(v.into())) {
            if !bool::from(scalar.is_zero()) {
                return Ok(scalar);
            }
        }
        // K = HMAC(K, V || 0x00)
        let mut buf = Vec::with_capacity(33);
        buf.extend_from_slice(&v);
        buf.push(0x00);
        k = mac(&k, &buf);
        v = mac(&k, &v);
    }
}

/// Sign the EIP-7702 authorization digest.
pub fn sign_authorization(
    auth: &Authorization,
    secret: &[u8; 32],
) -> Result<(u64, Vec<u8>, Vec<u8>)> {
    let address = crate::swap::decode_address(&auth.address)?;
    let mut stream = rlp::RlpStream::new_list(3);
    stream.append(&auth.chain_id);
    stream.append(&address);
    stream.append(&auth.nonce);
    let mut payload = vec![0x05u8];
    payload.extend_from_slice(stream.as_raw());
    let hash: [u8; 32] = Keccak256::digest(&payload).into();
    sign_hash(&hash, secret)
}

/// Sign a full type-0x04 set-code transaction; returns the raw hex.
pub fn sign_7702(tx: &Tx7702, secret: &[u8; 32]) -> Result<String> {
    let to = crate::swap::decode_address(&tx.to)?;

    // authorization_list (signed with the EOA key).
    let mut auth_encoded: Option<Vec<u8>> = None;
    if let Some(auth) = &tx.authorization {
        let (yp, r, s) = sign_authorization(auth, secret)?;
        let addr = crate::swap::decode_address(&auth.address)?;
        let mut a = rlp::RlpStream::new_list(6);
        a.append(&auth.chain_id);
        a.append(&addr);
        a.append(&auth.nonce);
        a.append(&yp);
        a.append(&r);
        a.append(&s);
        let mut list = rlp::RlpStream::new_list(1);
        list.append_raw(a.as_raw(), 1);
        auth_encoded = Some(list.as_raw().to_vec());
    }

    // unsigned payload
    let mut unsigned = rlp::RlpStream::new_list(10);
    unsigned.append(&tx.chain_id);
    unsigned.append(&tx.nonce);
    unsigned.append(&tx.max_priority_fee);
    unsigned.append(&tx.max_fee);
    unsigned.append(&tx.gas);
    unsigned.append(&to);
    unsigned.append(&tx.value);
    unsigned.append(&tx.data.clone());
    unsigned.begin_list(0); // access list
    match &auth_encoded {
        Some(raw) => unsigned.append_raw(raw, 1),
        None => unsigned.begin_list(0),
    };

    let mut payload = Vec::with_capacity(1 + unsigned.as_raw().len());
    payload.push(0x04);
    payload.extend_from_slice(unsigned.as_raw());
    let hash: [u8; 32] = Keccak256::digest(&payload).into();
    let (yp, r, s) = sign_hash(&hash, secret)?;

    // signed payload
    let mut signed = rlp::RlpStream::new_list(13);
    signed.append(&tx.chain_id);
    signed.append(&tx.nonce);
    signed.append(&tx.max_priority_fee);
    signed.append(&tx.max_fee);
    signed.append(&tx.gas);
    signed.append(&to);
    signed.append(&tx.value);
    signed.append(&tx.data.clone());
    signed.begin_list(0);
    match &auth_encoded {
        Some(raw) => signed.append_raw(raw, 1),
        None => signed.begin_list(0),
    };
    signed.append(&yp);
    signed.append(&r);
    signed.append(&s);

    let mut raw = Vec::with_capacity(1 + signed.as_raw().len());
    raw.push(0x04);
    raw.extend_from_slice(signed.as_raw());
    Ok(format!("0x{}", hex::encode(raw)))
}
/// Creation bytecode of [`SponsorAccount`] (solc 0.8.29, optimizer 200).
/// Constructor takes the sponsor address (abi-encoded, appended to the code).
pub const SPONSOR_ACCOUNT_BYTECODE: &str = "60a0604052348015600e575f5ffd5b50604051610371380380610371833981016040819052602b91603b565b6001600160a01b03166080526066565b5f60208284031215604a575f5ffd5b81516001600160a01b0381168114605f575f5ffd5b9392505050565b6080516102ee6100835f395f8181608d015260ee01526102ee5ff3fe608060405260043610610036575f3560e01c806354fd4d501461004157806377c936621461007c578063b61d27f6146100c7575f5ffd5b3661003d57005b5f5ffd5b34801561004c575f5ffd5b506040805180820190915260018152603160f81b60208201525b6040516100739190610200565b60405180910390f35b348015610087575f5ffd5b506100af7f000000000000000000000000000000000000000000000000000000000000000081565b6040516001600160a01b039091168152602001610073565b3480156100d2575f5ffd5b506100666100e1366004610219565b6060336001600160a01b037f000000000000000000000000000000000000000000000000000000000000000016148061011957503330145b61015a5760405162461bcd60e51b815260206004820152600e60248201526d1b9bdd08185d5d1a1bdc9a5e995960921b604482015260640160405180910390fd5b5f5f866001600160a01b03168686866040516101779291906102a9565b5f6040518083038185875af1925050503d805f81146101b1576040519150601f19603f3d011682016040523d82523d5f602084013e6101b6565b606091505b5091509150816101c857805160208201fd5b9695505050505050565b5f81518084528060208401602086015e5f602082860101526020601f19601f83011685010191505092915050565b602081525f61021260208301846101d2565b9392505050565b5f5f5f5f6060858703121561022c575f5ffd5b84356001600160a01b0381168114610242575f5ffd5b935060208501359250604085013567ffffffffffffffff811115610264575f5ffd5b8501601f81018713610274575f5ffd5b803567ffffffffffffffff81111561028a575f5ffd5b87602082840101111561029b575f5ffd5b949793965060200194505050565b818382375f910190815291905056fea26469706673582212200512036dfb43f248b178d58e5d9c55404f07ab567f1bac349bc09396d608cc5064736f6c634300081d0033";

/// Address used to CLEAR an EOA delegation (EIP-7702: zero address = clear).
pub const ZERO_DELEGATION: &str = "0x0000000000000000000000000000000000000000";

/// Deploy data for SponsorAccount: creation bytecode + sponsor address.
pub fn sponsor_deploy_data(sponsor_addr: &str) -> Result<Vec<u8>> {
    let mut data = hex::decode(SPONSOR_ACCOUNT_BYTECODE)?;
    let addr = crate::swap::decode_address(sponsor_addr)?;
    let mut arg = [0u8; 32];
    arg[12..].copy_from_slice(&addr);
    data.extend_from_slice(&arg);
    Ok(data)
}

/// Calldata for SponsorAccount.execute(address target, uint256 value, bytes data).
pub fn sponsor_execute_calldata(target: &str, value: u128, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = hex::decode("1cff79cd")?; // execute(address,uint256,bytes)
    let t = crate::swap::decode_address(target)?;
    let mut arg = [0u8; 32];
    arg[12..].copy_from_slice(&t);
    out.extend_from_slice(&arg);
    let mut val = [0u8; 32];
    val[16..].copy_from_slice(&value.to_be_bytes());
    out.extend_from_slice(&val);
    // bytes: offset, length, data (padded to 32)
    let mut off = [0u8; 32];
    off[28..].copy_from_slice(&96u32.to_be_bytes());
    out.extend_from_slice(&off);
    let mut len = [0u8; 32];
    len[28..].copy_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&len);
    out.extend_from_slice(data);
    let pad = (32 - data.len() % 32) % 32;
    out.extend_from_slice(&vec![0u8; pad]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vector generated with viem v2 (`account.signTransaction` with an
    /// eip7702 type + `account.signAuthorization`) for key 0x…01.
    #[test]
    fn golden_delegation_matches_viem() {
        let tx = Tx7702 {
            chain_id: 1,
            nonce: 0,
            max_priority_fee: 0x3b9aca00,
            max_fee: 0x2540be400,
            gas: 0x5208,
            to: "0x3535353535353535353535353535353535353535".to_string(),
            value: 0,
            data: vec![],
            authorization: Some(Authorization {
                chain_id: 1,
                address: "0x3535353535353535353535353535353535353535".to_string(),
                nonce: 0,
            }),
        };
        let secret: [u8; 32] =
            hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap()
                .try_into()
                .unwrap();
        let raw = sign_7702(&tx, &secret).unwrap();
        assert_eq!(
            raw,
            "0x04f8c90180843b9aca008502540be4008252089435353535353535353535353535353535353535358080c0f85cf85a019435353535353535353535353535353535353535358080a0daaed828509b2565fd21022336ed828408b07706cfca416d67304b6a68a44d5da02a30e775229d38755f04890cac2ecf1fce06c3038ea1a401c3e73382209d47f080a081ac3c2fcbe6a8f9458a071cb09892c95b384c4245fc818f3730559b614ccb81a021bf7b5a608b4e601d752bf07ef618a7a3789773300ca9900e7640343640bb83"
        );
    }

    #[test]
    fn execute_calldata_layout() {
        let data = sponsor_execute_calldata(
            "0x1234567890abcdef1234567890abcdef12345678",
            5,
            &[0xaa, 0xbb],
        )
        .unwrap();
        assert_eq!(&data[0..4], &[0x1c, 0xff, 0x79, 0xcd]);
        assert_eq!(
            &data[16..36],
            &hex::decode("1234567890abcdef1234567890abcdef12345678").unwrap()[..]
        );
        assert_eq!(&data[64..68], &[0u8, 0, 0, 5]);
    }
}
