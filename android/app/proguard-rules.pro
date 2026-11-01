# The Rust core, libqdp_android.so, exports JNI functions named after
# io.github.ianmarmour.qdp.Native and its methods
# (Java_io_github_ianmarmour_qdp_Native_connect and so on), so the class and
# its native methods keep their names and signatures. The core calls no
# Kotlin methods back, so nothing else needs keeping.
-keep class io.github.ianmarmour.qdp.Native {
    native <methods>;
}
