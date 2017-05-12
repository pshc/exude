#![recursion_limit = "1024"]

extern crate digest;
#[macro_use]
extern crate error_chain;
extern crate proto;
extern crate rpassword;
extern crate sha3;
extern crate sodiumoxide;

use std::env;
use std::io::{self, Read, Write};
use std::fs::{self, File};
use std::path::PathBuf;
use std::process;

use sha3::Shake128;
use sodiumoxide::crypto::{pwhash, secretbox, sign};

use proto::{Bincoded, Digest, DriverInfo, Signature};

mod errors {
    error_chain!{}
}
use errors::*;

fn main() {
    assert!(sodiumoxide::init());

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }

    if let Err(ref e) = dispatch(&args[1..]) {
        let stderr = &mut io::stderr();
        let errmsg = "Error writing to stderr";

        writeln!(stderr, "error: {}", e).expect(errmsg);

        for e in e.iter().skip(1) {
            writeln!(stderr, "caused by: {}", e).expect(errmsg);
        }

        process::exit(1);
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    match &*args[0] {
        "keygen" => keygen(),
        "sign" => sign(),
        cmd => {
            let _ = writeln!(io::stderr(), "Unknown command: {}", cmd);
            usage()
        }
    }
}

fn usage() -> ! {
    println!(
        "Command patterns:
    keygen
    sign <file>
"
    );
    process::exit(1)
}

const OPS: pwhash::OpsLimit = pwhash::OPSLIMIT_SENSITIVE;
const MEM: pwhash::MemLimit = pwhash::MEMLIMIT_SENSITIVE;

fn keygen() -> Result<()> {
    let dir = cred_path()?;
    println!("Keys will be written into: {}", dir.to_string_lossy());

    let password;
    {
        password = prompt_password("Please choose an encryption passphrase: ")?;
        let again = prompt_password("Please repeat it: ")?;
        ensure!(password == again, "passwords did not match");
    }

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
    if Err(()) == pwhash::derive_key(&mut box_key.0, password.as_bytes(), &salt, OPS, MEM) {
        bail!("not enough resources for pwhash");
    }

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

fn load_keys() -> Result<(sign::PublicKey, sign::SecretKey)> {
    let dir = cred_path()?;
    println!("Keys will be read from: {}", dir.to_string_lossy());

    let public_key;
    {
        let mut pub_bytes = [0; sign::PUBLICKEYBYTES];
        let eof = File::open(dir.join("public"))
            .and_then(
                |mut f| {
                    f.read_exact(&mut pub_bytes)?;
                    Ok(f.read(&mut [0u8])? == 0)
                },
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
                },
            )
            .chain_err(|| "couldn't load private key")?;
        nonce = secretbox::Nonce(nonce_bytes);
        salt = pwhash::Salt(salt_bytes);
    }

    let password = prompt_password("Passphrase: ")?;

    println!("Deriving encryption key...");
    let mut box_key = secretbox::Key([0; secretbox::KEYBYTES]);
    if Err(()) == pwhash::derive_key(&mut box_key.0, password.as_bytes(), &salt, OPS, MEM) {
        bail!("not enough resources for pwhash");
    }

    println!("Decrypting secret key...");
    let secret_key;
    {
        let mut plaintext = secretbox::open(&ciphertext, &nonce, &box_key)
            .map_err(|()| ErrorKind::Msg("invalid box key".into()))?;

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

fn sign() -> Result<()> {
    let mut root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root_path.pop();
    let root_path = root_path;

    let mut driver_path = root_path.clone();
    driver_path.push("driver");
    driver_path.push("target");
    driver_path.push("debug"); // xxx
    driver_path.push("libdriver.dylib"); // xxx
    let driver_path = driver_path;

    println!("Reading driver: {}", driver_path.display());
    let mut driver_bytes = Vec::new();
    File::open(driver_path)
        .and_then(|mut f| f.read_to_end(&mut driver_bytes))
        .chain_err(|| "could not read driver binary")?;
    let len = driver_bytes.len();

    println!("Hashing driver...");
    let digest = digest_from_bytes(&driver_bytes);

    let sig;
    {
        let (_, sk) = load_keys()?;
        println!("Signing driver...");
        sig = Signature(sign::sign_detached(&driver_bytes, &sk).0);
    }

    // write unsigned metadata
    // How do we prevent people from using old sigs to distribute old buggy drivers?
    // Expiry dates? Revocation?
    {
        let descriptor = DriverInfo { len: len, digest: digest, sig: sig };
        let bincoded = Bincoded::new(&descriptor)
            .chain_err(|| "driver metadata encoding issue")?;

        let descriptor_path = root_path.join("latest.meta");
        File::create(descriptor_path)
            .and_then(
                |mut file| {
                    file.write_all(bincoded.as_ref())?;
                    file.sync_all()
                },
            )
            .chain_err(|| "couldn't write metadata")?;
    }

    // temp: write a copy conveniently
    {
        let dest_path = root_path.join("latest.bin");
        File::create(dest_path)
            .and_then(
                |mut dest| {
                    dest.write_all(&driver_bytes)?;
                    dest.sync_all()
                },
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
    hasher.variable_result(&mut result).unwrap();
    Digest(result)
}

fn prompt_password(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().chain_err(|| "can't even")?;
    rpassword::read_password().chain_err(|| "can't hide password input")
}

fn cred_path() -> Result<PathBuf> {
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
