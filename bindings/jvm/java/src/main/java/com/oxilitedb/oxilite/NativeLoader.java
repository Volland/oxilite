package com.oxilitedb.oxilite;

import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * Loads the {@code oxilite_jvm} native library bundled as a classpath resource under
 * {@code /native/<os>-<arch>/}, extracting it to a temp file first since {@link System#load}
 * needs a real filesystem path (the standard approach for a JNI library shipped inside a jar,
 * as used by e.g. sqlite-jdbc and grpc-netty).
 */
final class NativeLoader {
    private static volatile boolean loaded = false;

    private NativeLoader() {}

    static synchronized void ensureLoaded() {
        if (loaded) {
            return;
        }
        String platform = detectPlatform();
        String resource = "/native/" + platform + "/" + libraryFileName();
        try (InputStream in = NativeLoader.class.getResourceAsStream(resource)) {
            if (in == null) {
                throw new UnsatisfiedLinkError(
                        "no oxilite native library bundled for platform " + platform
                                + " (looked for " + resource + " on the classpath)");
            }
            Path tmp = Files.createTempFile("liboxilite_jvm", suffix());
            tmp.toFile().deleteOnExit();
            Files.copy(in, tmp, StandardCopyOption.REPLACE_EXISTING);
            System.load(tmp.toAbsolutePath().toString());
        } catch (IOException e) {
            throw new UncheckedIOException("failed to extract the oxilite native library", e);
        }
        loaded = true;
    }

    private static String detectPlatform() {
        String os = normalizeOs(System.getProperty("os.name", ""));
        String arch = normalizeArch(System.getProperty("os.arch", ""));
        return os + "-" + arch;
    }

    private static String normalizeOs(String raw) {
        String os = raw.toLowerCase(java.util.Locale.ROOT);
        if (os.contains("mac") || os.contains("darwin")) {
            return "darwin";
        }
        if (os.contains("win")) {
            return "windows";
        }
        if (os.contains("linux")) {
            return "linux";
        }
        throw new UnsatisfiedLinkError("unsupported OS for oxilite: " + raw);
    }

    private static String normalizeArch(String raw) {
        String arch = raw.toLowerCase(java.util.Locale.ROOT);
        if (arch.equals("aarch64") || arch.equals("arm64")) {
            return "aarch64";
        }
        if (arch.equals("x86_64") || arch.equals("amd64")) {
            return "x86_64";
        }
        throw new UnsatisfiedLinkError("unsupported CPU architecture for oxilite: " + raw);
    }

    private static String libraryFileName() {
        String os = normalizeOs(System.getProperty("os.name", ""));
        if (os.equals("darwin")) {
            return "liboxilite_jvm.dylib";
        }
        if (os.equals("windows")) {
            return "oxilite_jvm.dll";
        }
        return "liboxilite_jvm.so";
    }

    private static String suffix() {
        String os = normalizeOs(System.getProperty("os.name", ""));
        if (os.equals("darwin")) {
            return ".dylib";
        }
        if (os.equals("windows")) {
            return ".dll";
        }
        return ".so";
    }
}
