extern crate digest;
extern crate proto;
extern crate rpassword;
extern crate sha3;
extern crate sodiumoxide;

use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::fs::{self, File};
use std::path::PathBuf;
use std::process;

use sha3::Shake128;
use sodiumoxide::crypto::{pwhash, secretbox, sign};

use proto::{Bincoded, Digest, DriverInfo, Signature};

fn main() {
    assert!(sodiumoxide::init());

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }
    if let Err(e) = dispatch(&args[1..]) {
        writeln!(io::stderr(), "Error: {}", e).unwrap();
        process::exit(1)
    }
}

fn dispatch(args: &[String]) -> io::Result<()> {
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
    println!("Command patterns:
    keygen
    sign <file>
");
    process::exit(1)
}

const OPS: pwhash::OpsLimit = pwhash::OPSLIMIT_SENSITIVE;
const MEM: pwhash::MemLimit = pwhash::MEMLIMIT_SENSITIVE;

fn keygen() -> io::Result<()> {
    let dir = cred_path();
    println!("Keys will be written into: {}", dir.to_string_lossy());

    let password;
    {
        print!("Please choose an encryption passphrase: ");
        io::stdout().flush()?;
        password = rpassword::read_password()?;
        print!("Please repeat it: ");
        io::stdout().flush()?;
        let again = rpassword::read_password()?;
        if password != again {
            return Err(io::Error::new(ErrorKind::InvalidInput, "passwords did not match"));
        }
    }

    let mut pub_file = fs::OpenOptions::new().write(true).create_new(true)
        .open(dir.join("public"))?;
    let mut priv_file = fs::OpenOptions::new().write(true).create_new(true)
        .open(dir.join("secret"))?;

    println!("Deriving encryption key...");
    let salt = pwhash::gen_salt();
    let mut box_key = secretbox::Key([0; secretbox::KEYBYTES]);
    if Err(()) == pwhash::derive_key(&mut box_key.0, password.as_bytes(), &salt, OPS, MEM) {
        return Err(io::Error::new(ErrorKind::Other, "not enough resources for pwhash"));
    }

    println!("Generating and encrypting key pair...");
    let (public_key, secret_key): (sign::PublicKey, sign::SecretKey) = sign::gen_keypair();
    verify_keys(&public_key, &secret_key)?;

    let nonce = secretbox::gen_nonce();
    let ciphertext = secretbox::seal(&secret_key.0, &nonce, &box_key);
    drop(secret_key);

    pub_file.write_all(&public_key.0)?;
    pub_file.sync_all()?;
    drop(pub_file);

    priv_file.write_all(&nonce.0)?;
    priv_file.write_all(&salt.0)?;
    priv_file.write_all(&ciphertext)?;
    priv_file.sync_all()?;
    drop(priv_file);

    println!("Keys written to disk.");
    Ok(())
}

fn load_keys() -> io::Result<(sign::PublicKey, sign::SecretKey)> {
    let dir = cred_path();
    println!("Keys will be read from: {}", dir.to_string_lossy());

    let public_key;
    {
        let mut pub_bytes = [0; sign::PUBLICKEYBYTES];
        let mut pub_file = File::open(dir.join("public"))?;
        pub_file.read_exact(&mut pub_bytes)?;
        if pub_file.read(&mut [0u8])? > 0 {
            return Err(io::Error::new(ErrorKind::InvalidData, "public key too long"));
        }
        public_key = sign::PublicKey(pub_bytes);
    }

    let nonce;
    let salt;
    let mut ciphertext = Vec::new();
    {
        let mut priv_file = File::open(dir.join("secret"))?;
        let mut nonce_bytes = [0; secretbox::NONCEBYTES];
        let mut salt_bytes = [0; pwhash::SALTBYTES];
        priv_file.read_exact(&mut nonce_bytes)?;
        priv_file.read_exact(&mut salt_bytes)?;
        priv_file.read_to_end(&mut ciphertext)?;
        nonce = secretbox::Nonce(nonce_bytes);
        salt = pwhash::Salt(salt_bytes);
    }

    print!("Passphrase: ");
    io::stdout().flush()?;
    let password = rpassword::read_password()?;

    println!("Deriving encryption key...");
    let mut box_key = secretbox::Key([0; secretbox::KEYBYTES]);
    if Err(()) == pwhash::derive_key(&mut box_key.0, password.as_bytes(), &salt, OPS, MEM) {
        return Err(io::Error::new(ErrorKind::Other, "not enough resources for pwhash"));
    }

    println!("Decrypting secret key...");
    let secret_key;
    {
        let mut plaintext = secretbox::open(&ciphertext, &nonce, &box_key)
            .map_err(|()| io::Error::new(ErrorKind::InvalidInput, "invalid box key"))?;

        if plaintext.len() != sign::SECRETKEYBYTES {
            return Err(io::Error::new(ErrorKind::InvalidData, "bad secret key length"));
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

fn verify_keys(pk: &sign::PublicKey, sk: &sign::SecretKey) -> io::Result<()> {
    let mut random = [0; 64];
    sodiumoxide::randombytes::randombytes_into(&mut random);
    let sig = sign::sign_detached(&random, &sk);
    if sign::verify_detached(&sig, &random, &pk) {
        Ok(())
    } else {
        Err(io::Error::new(ErrorKind::InvalidData, "could not verify keys"))
    }
}

fn sign() -> io::Result<()> {
    let mut root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root_path.pop();
    let root_path = root_path;

    let mut driver_path = root_path.clone();
    driver_path.push("target");
    driver_path.push("debug"); // xxx
    driver_path.push("libdriver.dylib"); // xxx
    let driver_path = driver_path;

    println!("Reading driver: {}", driver_path.to_string_lossy());
    let mut driver_bytes = Vec::new();
    File::open(driver_path)?.read_to_end(&mut driver_bytes)?;
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
        let descriptor = DriverInfo {
            len: len,
            digest: digest,
            sig: sig,
        };
        let bincoded = Bincoded::new(&descriptor)?;

        let mut descriptor_path = root_path.clone();
        descriptor_path.push("latest.meta");
        let mut file = File::create(descriptor_path)?;
        file.write_all(bincoded.as_ref())?;
        file.sync_all()?;
    }

    // temp: write a copy conveniently
    {
        let mut dest_path = root_path;
        dest_path.push("latest.bin");
        let mut dest = File::create(dest_path)?;
        dest.write_all(&driver_bytes)?;
        dest.sync_all()?;
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

fn cred_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("cred");

    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(ref e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => panic!("couldn't create {:?}: {}", path, e)
    }

    path
}

#[test]
fn test_cred_path() {
    let path = cred_path();
    assert!(path.ends_with("cred"));
    assert!(path.parent().unwrap().exists());
}
