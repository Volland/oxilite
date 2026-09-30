#!/usr/bin/env bash
# Generates a GPG key for signing Maven Central releases of oxilite-jvm, and publishes the
# public half to the keyservers Central checks against. Run this yourself, interactively — it
# asks for your name, email and a passphrase, and the private key never leaves this machine
# except when you explicitly export it in step 3 below to hand to GitHub Actions.
#
# See docs/java-publishing.md §4 for the full one-time Maven Central setup this is part of.
#
# Usage: bindings/jvm/scripts/generate-signing-key.sh

set -euo pipefail

if ! command -v gpg >/dev/null 2>&1; then
    echo "gpg not found. Install GnuPG first:" >&2
    echo "  macOS:   brew install gnupg" >&2
    echo "  Debian:  sudo apt-get install gnupg" >&2
    exit 1
fi

echo "This creates a GPG key to sign oxilite-jvm's release artifacts on Maven Central."
echo "The name and email you give are published to public keyservers permanently."
echo

read -rp "Real name for the key (e.g. your name or a project identity): " KEY_NAME
if [[ -z "$KEY_NAME" ]]; then
    echo "A name is required." >&2
    exit 1
fi

read -rp "Email for the key: " KEY_EMAIL
if [[ -z "$KEY_EMAIL" ]]; then
    echo "An email is required." >&2
    exit 1
fi

if gpg --list-secret-keys --with-colons "$KEY_EMAIL" >/dev/null 2>&1; then
    echo
    echo "A secret key for $KEY_EMAIL already exists:"
    gpg --list-secret-keys --keyid-format=long "$KEY_EMAIL"
    read -rp "Generate another one anyway? [y/N] " CONFIRM
    if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
        echo "Aborted. Reuse the existing key's id in the steps below."
        exit 0
    fi
fi

echo
echo "gpg will now ask you to confirm the key parameters and choose a passphrase."
echo "Recommended: RSA and RSA, 4096 bits, key does not expire (or a long expiry you'll renew)."
echo

gpg --full-generate-key

echo
echo "Looking up the key you just created..."
KEY_ID=$(gpg --list-secret-keys --with-colons --keyid-format=long "$KEY_EMAIL" \
    | awk -F: '/^sec/ { print $5; exit }')

if [[ -z "$KEY_ID" ]]; then
    echo "Could not find a key for $KEY_EMAIL. List keys with 'gpg --list-secret-keys' and rerun" >&2
    echo "the keyserver step manually: gpg --keyserver keyserver.ubuntu.com --send-keys <KEY_ID>" >&2
    exit 1
fi

echo "Key id: $KEY_ID"
echo
echo "Publishing the public key to keyservers Maven Central checks against..."
gpg --keyserver keyserver.ubuntu.com --send-keys "$KEY_ID"
gpg --keyserver keys.openpgp.org --send-keys "$KEY_ID"

cat <<EOF

Done. Next steps (docs/java-publishing.md §4, steps 6-7):

1. Export the private key, to hand to GitHub Actions as a secret:

     gpg --armor --export-secret-keys $KEY_ID > oxilite-signing-key.asc

2. Add the GitHub Actions secrets (needs the 'maven-central' environment created first, and
   your Central Portal User Token from central.sonatype.com):

     gh secret set GPG_PRIVATE_KEY --env maven-central < oxilite-signing-key.asc
     gh secret set GPG_PASSPHRASE  --env maven-central
     gh secret set MAVEN_CENTRAL_USERNAME --env maven-central
     gh secret set MAVEN_CENTRAL_PASSWORD --env maven-central

3. Delete the exported private key file once the secret is set:

     shred -u oxilite-signing-key.asc   # or: rm oxilite-signing-key.asc

Key id for reference: $KEY_ID
EOF
