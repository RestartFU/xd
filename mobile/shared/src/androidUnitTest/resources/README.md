# Loopback TLS fixture

`loopback.p12` is a test-only key and self-signed certificate used solely by
`AndroidSocketTest` to run a local JSSE server. Its password is the public test
value `changeit`; it is not trusted by, copied into, or referenced from any app
or release build.

Regenerate it with a JDK `keytool`:

```sh
keytool -genkeypair \
  -alias xd-test \
  -keyalg RSA \
  -keysize 2048 \
  -validity 3650 \
  -dname "CN=localhost" \
  -storetype PKCS12 \
  -keystore loopback.p12 \
  -storepass changeit \
  -keypass changeit
```
