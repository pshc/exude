#![feature(box_syntax)]
#![recursion_limit = "1024"]

extern crate digest;
#[macro_use]
extern crate error_chain;
extern crate proto;
extern crate rpassword;
extern crate sha3;
extern crate sodiumoxide;

use std::io::{self, Read, Write};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use sha3::Shake128;
use sodiumoxide::crypto::{pwhash, secretbox, sign};

use proto::{Bincoded, Digest, DriverInfo, Signature};
pub use secret::Secret;

pub mod errors {
    error_chain! {
        errors { InvalidPassword }
    }
}
pub use errors::*;

const OPS: pwhash::OpsLimit = pwhash::OPSLIMIT_SENSITIVE;
const MEM: pwhash::MemLimit = pwhash::MEMLIMIT_SENSITIVE;

pub type InsecureKeys = (sign::PublicKey, sign::SecretKey);

pub fn keygen(dir: &Path, password: Secret) -> Result<()> {
    assert!(sodiumoxide::init());

    let mut pub_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("public"))
        .chain_err(|| "unable to create new public key")?;
    let mut priv_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("secret"))
        .chain_err(|| "unable to create new secret key")?;

    println!("Deriving encryption key...");
    let salt = pwhash::gen_salt();
    let mut box_key = secretbox::Key([0; secretbox::KEYBYTES]);
    if Err(()) == password.expose(|b| pwhash::derive_key(&mut box_key.0, b, &salt, OPS, MEM)) {
        bail!("not enough resources for pwhash");
    }
    drop(password);

    println!("Generating and encrypting key pair...");
    let (public_key, secret_key): (sign::PublicKey, sign::SecretKey) = sign::gen_keypair();
    verify_keys(&public_key, &secret_key)?;

    let nonce = secretbox::gen_nonce();
    let ciphertext = secretbox::seal(&secret_key.0, &nonce, &box_key);
    drop(secret_key);

    pub_file.write_all(&public_key.0)
        .and_then(|()| pub_file.sync_all())
        .chain_err(|| "couldn't write public key")?;
    drop(pub_file);

    Ok(())
        .and_then(|()| {
            priv_file.write_all(&nonce.0)?;
            priv_file.write_all(&salt.0)?;
            priv_file.write_all(&ciphertext)?;
            priv_file.sync_all()
        })
        .chain_err(|| "couldn't write private key")?;

    drop(priv_file);

    println!("Keys written to disk.");
    Ok(())
}

pub fn load_keys() -> Result<InsecureKeys> {
    let dir = cred_path()?;
    println!("Keys will be read from: {}", dir.display());

    let public_key;
    {
        let mut pub_bytes = [0; sign::PUBLICKEYBYTES];
        let eof = File::open(dir.join("public"))
            .and_then(
                |mut f| {
                    f.read_exact(&mut pub_bytes)?;
                    Ok(f.read(&mut [0u8])? == 0)
                }
            )
            .chain_err(|| "couldn't load public key")?;
        ensure!(eof, "public key too long");
        public_key = sign::PublicKey(pub_bytes);
    }

    let nonce;
    let salt;
    let mut ciphertext = Vec::new();
    {
        let mut nonce_bytes = [0; secretbox::NONCEBYTES];
        let mut salt_bytes = [0; pwhash::SALTBYTES];
        File::open(dir.join("secret"))
            .and_then(
                |mut f| {
                    f.read_exact(&mut nonce_bytes)?;
                    f.read_exact(&mut salt_bytes)?;
                    f.read_to_end(&mut ciphertext)
                }
            )
            .chain_err(|| "couldn't load private key")?;
        nonce = secretbox::Nonce(nonce_bytes);
        salt = pwhash::Salt(salt_bytes);
    }

    let password = Secret::from_user_input("Passphrase: ")?;

    println!("Deriving encryption key...");
    let mut box_key = secretbox::Key([0; secretbox::KEYBYTES]);
    if Err(()) == password.expose(|b| pwhash::derive_key(&mut box_key.0, b, &salt, OPS, MEM)) {
        bail!("not enough resources for pwhash");
    }

    println!("Decrypting secret key...");
    let secret_key;
    {
        let mut plaintext = secretbox::open(&ciphertext, &nonce, &box_key)
            .map_err(|()| -> Error { ErrorKind::InvalidPassword.into() })?;

        if plaintext.len() != sign::SECRETKEYBYTES {
            bail!("bad secret key length ({})", plaintext.len());
        }
        let mut secret_bytes = [0; sign::SECRETKEYBYTES];
        secret_bytes.copy_from_slice(&plaintext);
        secret_key = sign::SecretKey(secret_bytes);
        sodiumoxide::utils::memzero(&mut secret_bytes);
        sodiumoxide::utils::memzero(&mut plaintext);
    }

    verify_keys(&public_key, &secret_key)?;

    Ok((public_key, secret_key))
}

fn verify_keys(pk: &sign::PublicKey, sk: &sign::SecretKey) -> Result<()> {
    let mut random = [0; 64];
    sodiumoxide::randombytes::randombytes_into(&mut random);
    let sig = sign::sign_detached(&random, &sk);
    let verified = sign::verify_detached(&sig, &random, &pk);
    ensure!(verified, "could not verify keys");
    Ok(())
}

pub fn sign(driver_path: &Path, keys: &InsecureKeys, out_dir: &Path) -> Result<()> {
    assert!(sodiumoxide::init());

    println!("Reading driver: {}", driver_path.display());
    let mut driver_bytes = Vec::new();
    File::open(driver_path)
        .and_then(|mut f| f.read_to_end(&mut driver_bytes))
        .chain_err(|| "could not read driver binary")?;
    let len = driver_bytes.len();

    println!("Hashing driver...");
    let digest = digest_from_bytes(&driver_bytes);

    println!("Signing driver...");
    let sig = Signature(sign::sign_detached(&driver_bytes, &keys.1).0);

    // write unsigned metadata
    // How do we prevent people from using old sigs to distribute old buggy drivers?
    // Expiry dates? Revocation?
    {
        let descriptor = DriverInfo { len: len, digest: digest, sig: sig };
        let bincoded = Bincoded::new(&descriptor)
            .chain_err(|| "driver metadata encoding issue")?;

        let descriptor_path = out_dir.join("latest.meta");
        File::create(descriptor_path)
            .and_then(
                |mut file| {
                    file.write_all(bincoded.as_ref())?;
                    file.sync_all()
                }
            )
            .chain_err(|| "couldn't write metadata")?;
    }

    // temp: write a copy conveniently
    {
        let dest_path = out_dir.join("latest.bin");
        File::create(dest_path)
            .and_then(
                |mut dest| {
                    dest.write_all(&driver_bytes)?;
                    dest.sync_all()
                }
            )
            .chain_err(|| "couldn't copy driver")?;
    }

    println!("Wrote signature.");

    Ok(())
}

fn digest_from_bytes(bytes: &[u8]) -> Digest {
    use digest::{Input, VariableOutput};

    let mut hasher = Shake128::default();
    hasher.digest(bytes);
    let mut result = [0u8; proto::digest::LEN];
    hasher.variable_result(&mut result).expect("hashing");
    Digest(result)
}

pub mod secret {
    use std::fmt;
    use std::io::{self, Write};
    use sodiumoxide::utils::{memcmp, memzero};
    use rpassword;
    use errors::*;

    #[derive(Clone)]
    pub struct Secret(Box<[u8]>);

    impl Secret {
        pub fn from_user_input(prompt: &str) -> Result<Self> {
            print!("{}", prompt);
            io::stdout().flush().chain_err(|| "can't even")?;
            let s = rpassword::read_password()
                .chain_err(|| "can't hide password input")?;
            let pass = Secret(s.into_bytes().into_boxed_slice());
            Ok(pass)
        }

        pub fn expose<F: FnOnce(&[u8]) -> T, T>(&self, f: F) -> T {
            f(&*self.0)
        }
    }

    impl fmt::Debug for Secret {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("Secret(****)")
        }
    }

    impl PartialEq for Secret {
        fn eq(&self, other: &Secret) -> bool {
            memcmp(&*self.0, &*other.0)
        }
    }

    impl Drop for Secret {
        fn drop(&mut self) {
            memzero(&mut self.0);
        }
    }
}

pub fn cred_path() -> Result<PathBuf> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("cred");

    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(ref e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => Err(e).chain_err(|| "couldn't create cred dir")?,
    }

    Ok(path)
}

#[test]
fn test_cred_path() {
    let path = cred_path().unwrap();
    assert!(path.ends_with("cred"));
    assert!(path.parent().unwrap().exists());
}
