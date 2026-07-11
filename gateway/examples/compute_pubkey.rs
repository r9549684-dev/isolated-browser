use x25519_dalek::{StaticSecret, PublicKey};

fn main() {
    let secret_hex = std::env::args().nth(1).expect("need hex secret");
    let mut secret_bytes = [0u8; 32];
    for (i, chunk) in secret_hex.as_bytes().chunks(2).enumerate() {
        let byte_str = std::str::from_utf8(chunk).unwrap();
        secret_bytes[i] = u8::from_str_radix(byte_str, 16).unwrap();
    }
    let secret = StaticSecret::from(secret_bytes);
    let public = PublicKey::from(&secret);
    let hex_str: String = public.as_bytes().iter().map(|b| format!("{:02x}", b)).collect();
    println!("{}", hex_str);
}
