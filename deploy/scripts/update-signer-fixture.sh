#!/usr/bin/env bash
# Offline androidTest asset: real APK, same package, higher code, deliberately unrelated throwaway signer.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$root"
sdk=${1:?SDK required}; platform_jar=${2:?platform jar required}; tools=${3:?build-tools required}
java=${4:?JDK required}; output=${5:?asset output required}
[[ $output == "$root/client/androidApp/build/"* ]] || { printf 'test assets must be in the app build directory\n' >&2; exit 2; }
mkdir -m 700 -p "$root/var/update-signer-fixture"
work=$(mktemp -d "$root/var/update-signer-fixture/build.XXXXXXXX")
cleanup() { local code=$?; trap - EXIT INT TERM; rm -rf -- "$work"; exit "$code"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# Key/password never enter build/ or the test APK. The generated asset contains only the public certificate.
printf 'fixture-only-password\n' > "$work/key.pass"
printf '%s\n' '<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="app.porchlight" android:versionCode="2100000000" android:versionName="210000.0.0"><uses-sdk android:minSdkVersion="28" android:targetSdkVersion="35"/><application android:hasCode="false" android:label="Update signer fixture"/></manifest>' > "$work/AndroidManifest.xml"
"$java/bin/keytool" -genkeypair -noprompt -alias fixture -keyalg RSA -keysize 2048 -validity 30 \
  -keystore "$work/fixture.jks" -storepass:file "$work/key.pass" -keypass:file "$work/key.pass" \
  -dname 'CN=Update Signer Fixture' > "$work/keytool.log" 2>&1
"$sdk/build-tools/$tools/aapt2" link -I "$platform_jar" \
  --manifest "$work/AndroidManifest.xml" -o "$work/unsigned.apk"
cp "$work/key.pass" "$work/private-key.pass"
"$sdk/build-tools/$tools/apksigner" sign --ks "$work/fixture.jks" --ks-key-alias fixture \
  --ks-pass "file:$work/key.pass" --key-pass "file:$work/private-key.pass" --out "$work/wrong-signer.apk" "$work/unsigned.apk"
"$sdk/build-tools/$tools/apksigner" verify "$work/wrong-signer.apk"
mkdir -p "$output"
cp "$work/wrong-signer.apk" "$output/wrong-signer.apk"
