# Building and publishing the JVM package

This guide takes you from a fresh checkout to a published `com.oxilitedb:oxilite-jvm` release on
Maven Central. The package lives in [`bindings/jvm`](../bindings/jvm). For its API, see the
[package README](../bindings/jvm/README.md).

It covers five things:
1. Setting up a development environment.
2. Building and testing the package.
3. Building release artifacts locally.
4. The one-time Maven Central and GPG setup.
5. Releasing, either automatically from a tag or by hand.

## How the package is put together

| Piece | Where | Role |
|---|---|---|
| Native library | `bindings/jvm/src/` (Rust crate `oxilite-jvm`, the `jni` crate) | Wraps `oxilite::blocking::Store`, exchanging JSON with Java over JNI |
| Java package `com.oxilitedb.oxilite` | `bindings/jvm/java/src/main/java/` | `Store`, the RDF term/quad model, the exception hierarchy |
| Native libraries in the jar | `bindings/jvm/java/src/main/resources/native/<os>-<arch>/` | One per platform; `NativeLoader` picks the right one at runtime |
| Build configuration | `bindings/jvm/java/pom.xml` | Maven; the `release` profile adds sources/javadoc/signing/publishing |
| Tests | `bindings/jvm/java/src/test/java/` | JUnit 5, against the real native library |
| CI | `.github/workflows/ci.yml`, job `jvm` | Builds and tests on Linux for every push |
| Release | `.github/workflows/java-release.yml` | Builds every platform's native library; stages a Central deployment on `v*` tags |

Unlike the Python wheels (one per platform) or the npm package (platform detection via optional
dependencies), Maven has no first-class multi-platform package mechanism, so **one jar bundles
every platform's native library** as a resource — the same approach `sqlite-jdbc` and
`grpc-netty` use. `NativeLoader` extracts the matching one to a temp file and `System.load()`s
it. This means a full release needs the native library built on every target platform before the
jar is assembled; see [3. Build release artifacts locally](#3-build-release-artifacts-locally)
and the `native` job of `java-release.yml`.

## 1. Set up a development environment

You need three things:
- **Rust**: the stable toolchain from [rustup](https://rustup.rs). The workspace needs rustc
  1.88 or later.
- **A JDK**: 17 or later for building (the compiled bytecode targets Java 11, so the *published*
  jar still runs on Java 11+; building it needs a newer `javac`). Temurin is what CI uses.
- **Maven**: 3.9 or later.

```bash
git clone https://github.com/Volland/oxilite.git
cd oxilite/bindings/jvm
```

> **Two `rustc`s on one machine.** If another `rustc` is also installed (for example Homebrew's),
> put rustup's first on your `PATH` with `export PATH="$HOME/.cargo/bin:$PATH"`. Otherwise the
> build can fail with `rustc … is not supported`.

## 2. Build and test

```bash
./scripts/build-native.sh          # debug build, fast to compile
./scripts/build-native.sh --release  # optimized build, what a release uses
cd java && mvn test
```

`build-native.sh` builds the `oxilite-jvm` cdylib for the host platform and copies it into
`java/src/main/resources/native/<os>-<arch>/`, where `NativeLoader` (and the test suite) expects
to find it. After changing a `.java` file, `mvn test` alone is enough. After changing anything
under `bindings/jvm/src/`, rerun `build-native.sh` first.

Lint the Rust side from the repository root:

```bash
cargo fmt -p oxilite-jvm
cargo clippy -p oxilite-jvm --all-targets
```

CI runs the same steps (on Linux only) in the `jvm` job of `.github/workflows/ci.yml`.

## 3. Build release artifacts locally

```bash
cd bindings/jvm
./scripts/build-native.sh --release
cd java
mvn -P release package -DskipTests   # oxilite-jvm-X.Y.Z.jar, -sources.jar, -javadoc.jar in target/
```

The `release` profile (only active with `-P release`, so a plain `mvn test` never needs a GPG
key) adds the sources jar, the javadoc jar, GPG signing, and the Central Publishing plugin.
Building locally like this only bundles **your** platform's native library — a real release
needs every platform's, which is what the `native` job of `java-release.yml` provides before
`publish` runs `mvn -P release deploy`. There is no equivalent to `maturin build`'s
cross-compilation here; the workflow's matrix is the tested path.

Check the jar in a clean project:

```bash
mkdir -p /tmp/oxilite-jvm-check && cd /tmp/oxilite-jvm-check
cat > pom.xml <<'EOF'
<project><modelVersion>4.0.0</modelVersion>
  <groupId>check</groupId><artifactId>check</artifactId><version>1</version>
  <dependencies>
    <dependency><groupId>com.oxilitedb</groupId><artifactId>oxilite-jvm</artifactId>
      <version>X.Y.Z</version><scope>system</scope>
      <systemPath>${project.basedir}/oxilite-jvm.jar</systemPath></dependency>
  </dependencies>
</project>
EOF
cp <path-to>/target/oxilite-jvm-X.Y.Z.jar oxilite-jvm.jar
# then write a one-line Main.java that opens a Store and run it with -cp
```

## 4. One-time Maven Central and GPG setup

Unlike PyPI's trusted publishing (no secret stored, GitHub proves its identity via OIDC), Maven
Central's Central Portal is set up with a **user token** (a username/password pair) plus a
**GPG key** that signs every artifact. Both go into GitHub secrets once; nothing here can be
done on your behalf; each step below is something you run yourself.

1. **Create a Central Portal account.** Sign up at
   [central.sonatype.com](https://central.sonatype.com). It can authenticate with a GitHub
   account.
2. **Register and verify the `com.oxilitedb` namespace.** In the Central Portal, go to
   *Namespaces → Add Namespace*, enter `com.oxilitedb`, and follow the domain verification flow:
   it gives you a TXT record value to add to `oxilitedb.com`'s DNS (`_sonatype.oxilitedb.com` or
   similar — the portal shows the exact name). Verification can take a few minutes to a few
   hours depending on DNS propagation. This step alone can only be done by whoever controls
   oxilitedb.com's DNS.
3. **Generate a User Token.** In the Central Portal, go to *your account → Generate User Token*.
   This gives you a username and a password-like token — treat it like a password; it is what
   Maven authenticates with, not your login password.
4. **Generate a GPG key and publish it**, on your own machine (never share the private key,
   including with an AI assistant — this step has to be run by you, interactively, so the
   passphrase is never typed anywhere else):

   ```bash
   bindings/jvm/scripts/generate-signing-key.sh
   ```

   It asks for a name and email (published to public keyservers permanently), runs
   `gpg --full-generate-key` (recommended: RSA and RSA, 4096 bits, no expiry or a long one),
   and publishes the public key to `keyserver.ubuntu.com` and `keys.openpgp.org`. It prints the
   key id and the exact commands for the next two steps when it's done.

5. **Export the private key** for GitHub Actions to import at build time:

   ```bash
   gpg --armor --export-secret-keys <KEY_ID> > oxilite-signing-key.asc
   ```

6. **Create the `maven-central` GitHub environment and its secrets.** In the repository, go to
   *Settings → Environments → New environment*, name it `maven-central`. Under *Deployment
   branches and tags*, restrict it to the tag pattern `v*` and branch `main`, and consider adding
   yourself as a required reviewer (each publish then waits for your approval in the Actions
   tab) — the same safety pattern `docs/python-publishing.md` uses for PyPI. Then add these
   repository or environment secrets:

   | Secret | Value |
   |---|---|
   | `MAVEN_CENTRAL_USERNAME` | The username half of the User Token from step 3 |
   | `MAVEN_CENTRAL_PASSWORD` | The token/password half of the User Token from step 3 |
   | `GPG_PRIVATE_KEY` | The full contents of `oxilite-signing-key.asc` from step 5 |
   | `GPG_PASSPHRASE` | The passphrase you set in step 4 |

   ```bash
   gh secret set MAVEN_CENTRAL_USERNAME --env maven-central
   gh secret set MAVEN_CENTRAL_PASSWORD --env maven-central
   gh secret set GPG_PRIVATE_KEY --env maven-central < oxilite-signing-key.asc
   gh secret set GPG_PASSPHRASE --env maven-central
   ```

7. **Delete the local export** once the secret is set: `shred -u oxilite-signing-key.asc` (or
   just `rm` if `shred` is unavailable).

After this, `bindings/jvm/java/pom.xml`'s `release` profile and `java-release.yml` have
everything they need. `autoPublish` is `false` in the `central-publishing-maven-plugin`
configuration on purpose (see the comment in `pom.xml`): a Central release cannot be deleted or
overwritten once public, so every deployment lands as a reviewable, unpublished "deployment" at
<https://central.sonatype.com/publishing/deployments> first. Once you trust the pipeline, flip
it to `true` to skip that manual click.

## 5. Release

### Automatically, from the release tag

A release is tagged in the usual way:
1. Bump the workspace version in the root `Cargo.toml` (`bindings/jvm/java/pom.xml`'s
   `<version>` is not tied to the workspace version automatically — bump it by hand to match).
2. Commit "Release X.Y.Z".
3. Create an annotated tag `vX.Y.Z` and push it.

Pushing the tag starts `java-release.yml`:
1. **`native`** builds the native library on five platforms (`linux-x86_64`, `linux-aarch64`,
   `darwin-x86_64`, `darwin-aarch64`, `windows-x86_64`) and uploads each as an artifact.
2. **`publish`** downloads every platform's native library into
   `java/src/main/resources/native/<platform>/`, so the built jar bundles all five, then runs
   `mvn -P release deploy`: builds the jar, sources jar and javadoc jar, signs all three with
   GPG, and uploads the signed bundle to Central. If you added a required reviewer, approve the
   deployment in the Actions tab.

Then go to <https://central.sonatype.com/publishing/deployments>, check the staged deployment
(its contents, POM metadata, that every platform's native library is present — the jar should
be a few MB per platform larger than a single-platform build), and click **Publish**. It takes
up to 30 minutes to appear as searchable on Central and sync to `repo.maven.apache.org`; the
Central Portal's *Deployments* page itself resolves it almost immediately with the exact
coordinates (`com.oxilitedb:oxilite-jvm:X.Y.Z`) to `mvn install`/`pom.xml` dependency against.

### When the tag run fails

A tag cannot be rebuilt with a fix, and moving a pushed tag is bad practice. If a platform's
native build fails on the tag (so nothing was published), fix it on `main`, then run the
workflow by hand from `main` (*Actions → Java release → Run workflow*, branch `main`) with
**Stage a deployment to Maven Central** checked, or:

```bash
gh workflow run java-release.yml --ref main -f publish=true
```

Do it only while `main` has not moved on to the next version's changes in the JVM binding.

### By hand, without the workflow

This path is for emergencies, or for a platform the workflow does not build.

1. Build the native library on every platform you want to ship (or reuse jars from a previous
   run's artifacts for the platforms you cannot build locally) and place each one under
   `java/src/main/resources/native/<os>-<arch>/`.
2. Ensure `~/.m2/settings.xml` has a `central` server with your User Token:

   ```xml
   <settings>
     <servers>
       <server>
         <id>central</id>
         <username>YOUR_TOKEN_USERNAME</username>
         <password>YOUR_TOKEN_PASSWORD</password>
       </server>
     </servers>
   </settings>
   ```

3. Run:

   ```bash
   cd bindings/jvm/java
   export GPG_TTY=$(tty)   # gpg-agent needs a terminal to prompt for the passphrase
   mvn -P release deploy
   ```

A version cannot be republished once public on Central, only deployed as a new version, so a fix
means a version bump.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `rustc 1.87.0 is not supported` | An older `rustc` is first on `PATH`. Use rustup's (`export PATH="$HOME/.cargo/bin:$PATH"`) |
| `UnsatisfiedLinkError: no oxilite native library bundled for platform …` | The jar has no native library for the running platform. Rebuild the jar after running `build-native.sh` on (or for) that platform |
| `gpg: signing failed: No such file or directory` in CI | `gpg-private-key`/`gpg-passphrase` were not passed to `actions/setup-java`, or the secret is empty |
| `401` from Central during `deploy` | `MAVEN_CENTRAL_USERNAME`/`PASSWORD` are wrong, or the User Token was regenerated (tokens can be revoked) |
| `403` / `namespace not authorized` from Central | The `com.oxilitedb` namespace is not yet verified on this account, or the account used to deploy is not the one that verified it |
| The deployment sits in *Deployments* forever | `autoPublish` is `false` by default; click **Publish** yourself at central.sonatype.com/publishing/deployments |
| `readme.md not found` or similar during `javadoc:jar` | The javadoc plugin ran against sources missing package-level docs; this is a warning, not a failure, unless `doclint` is stricter than the default |
