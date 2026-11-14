# R8 rules for the release build, on top of proguard-android-optimize.txt and
# the rules each library ships (AndroidX, Compose, coroutines and
# kotlinx.serialization bring their own, including the serializers'
# Companion.serializer() that a generic decode looks up).

# JSch, for SSH hosts, picks its ciphers, key exchanges, signatures and
# key-pair readers by class name from its configuration
# (JSch.getConfig -> Class.forName), so none of them is reachable from code
# R8 can see. Keep the library whole; the Bouncy Castle classes its bc
# package uses directly are then kept through those references.
-keep class com.jcraft.jsch.** { *; }

# Optional integrations JSch reaches only when they are on the class path,
# and none is on Android's: Windows' Pageant agent (JNA), Kerberos (GSS-API),
# Unix-domain sockets (junixsocket), and log4j/SLF4J logging. R8 reports
# them as missing; nothing on this app's paths calls them.
-dontwarn com.sun.jna.**
-dontwarn org.ietf.jgss.**
-dontwarn org.newsclub.net.unix.**
-dontwarn org.apache.logging.log4j.**
-dontwarn org.slf4j.**

# Readable stack traces from the field: keep line numbers, hide source names.
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile
