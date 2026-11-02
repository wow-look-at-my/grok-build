// crypto_provider_preinstalled.rs holds the crypto provider preinstalled code.

#[test]
fn earlier_install_is_preserved() {
    own_process!(earlier_install_is_preserved);
    // One cipher suite marks the host's provider apart from the stock ring provider.
    let mut host = rustls::crypto::ring::default_provider();
    host.cipher_suites.truncate(1);
    host.install_default().expect("first install wins");
    xai_grok_extra_ca::ensure_default_crypto_provider();
    let provider = rustls::crypto::CryptoProvider::get_default().expect("default installed");
    assert_eq!(
        provider.cipher_suites.len(),
        1,
        "ensure must not replace the host's provider"
    );
}
