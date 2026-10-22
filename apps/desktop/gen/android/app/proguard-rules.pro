# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile
# Kagaz's Rust code calls these by name (mobile.rs): the ring, the saved
# sign-in and the fans' Bluetooth, some with the app's window closed.
# Nothing in Kotlin calls them, so without this the shrinker drops them
# (it dropped bleScan and the rest on 2026-10-22). Every public static
# method, so a new one cannot be forgotten here.
-keep class life.omniverse.kagaz.KagazNative {
  public static *;
}
