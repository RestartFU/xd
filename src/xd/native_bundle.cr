module Xd
  # Release gate shared by macOS and Windows native packagers. A native
  # artifact must carry the same app-owned capabilities as the Linux bundle;
  # finding a tool or library on the build runner does not satisfy the gate.
  module NativeBundle
    extend self

    enum Platform
      MacOS
      Windows
    end

    record Requirement,
      label : String,
      alternatives : Array(String)

    def parse_platform(value : String) : Platform?
      case value
      when "macos"   then Platform::MacOS
      when "windows" then Platform::Windows
      end
    end

    def validate(root : String, platform : Platform) : Array(String)
      requirements(platform).compact_map do |requirement|
        requirement.label unless requirement.alternatives.any? do |pattern|
                                   matches_file?(File.join(root, pattern))
                                 end
      end
    end

    def requirements(platform : Platform) : Array(Requirement)
      platform.mac_os? ? macos_requirements : windows_requirements
    end

    private def matches_file?(pattern : String) : Bool
      if pattern.includes?('*') || pattern.includes?('?') ||
         pattern.includes?('[')
        Dir.glob(pattern).any? { |path| File.file?(path) }
      else
        File.file?(pattern)
      end
    rescue File::Error
      false
    end

    private def macos_requirements : Array(Requirement)
      resources = "Contents/Resources"
      [
        requirement("Crystal executable", "Contents/MacOS/xd"),
        requirement(
          "compiled GSettings schemas",
          "#{resources}/share/glib-2.0/schemas/gschemas.compiled"
        ),
        requirement("DM Sans", "#{resources}/share/fonts/xd/DMSans-Variable.ttf"),
        requirement("application icon", "#{resources}/xd.icns"),
        requirement("GIO TLS module", "#{resources}/lib/gio/modules/*.so"),
        requirement(
          "pixbuf loader",
          "#{resources}/lib/gdk-pixbuf-2.0/*/loaders/*.so"
        ),
        requirement(
          "pixbuf loader cache",
          "#{resources}/etc/gdk-pixbuf-loaders.cache",
          "#{resources}/etc/gdk-pixbuf-loaders.cache.in",
          "#{resources}/lib/gdk-pixbuf-2.0/*/loaders.cache"
        ),
        requirement("VTE runtime", "#{resources}/lib/libvte*.dylib"),
        requirement("PortAudio runtime", "#{resources}/lib/libportaudio*.dylib"),
        requirement("SQLite runtime", "#{resources}/lib/libsqlite3*.dylib"),
        requirement(
          "bundled Git",
          "#{resources}/git/bin/git",
          "#{resources}/bin/git"
        ),
        requirement(
          "Git HTTPS transport",
          "#{resources}/git/libexec/git-core/git-remote-https",
          "#{resources}/libexec/git-core/git-remote-https"
        ),
        requirement(
          "Git certificate authorities",
          "#{resources}/git/ssl/certs/ca-bundle.crt",
          "#{resources}/etc/ssl/certs/ca-certificates.crt"
        ),
        requirement(
          "Codex",
          "#{resources}/libexec/codex",
          "#{resources}/libexec/codex-package/bin/codex"
        ),
        requirement("Claude", "#{resources}/libexec/claude"),
        requirement("whisper.cpp", "#{resources}/libexec/whisper"),
        requirement("OpenSSL", "#{resources}/libexec/openssl"),
      ]
    end

    private def windows_requirements : Array(Requirement)
      [
        requirement("Crystal executable", "bin/xd.exe"),
        requirement(
          "compiled GSettings schemas",
          "share/glib-2.0/schemas/gschemas.compiled"
        ),
        requirement("DM Sans", "share/fonts/xd/DMSans-Variable.ttf"),
        requirement("application icon", "share/icons/hicolor/scalable/apps/*.svg"),
        requirement("GIO TLS module", "lib/gio/modules/*.dll"),
        requirement(
          "pixbuf loader",
          "lib/gdk-pixbuf-2.0/*/loaders/*.dll"
        ),
        requirement(
          "pixbuf loader cache",
          "etc/gdk-pixbuf-loaders.cache",
          "etc/gdk-pixbuf-loaders.cache.in",
          "lib/gdk-pixbuf-2.0/*/loaders.cache"
        ),
        requirement("VTE runtime", "bin/libvte*.dll"),
        requirement("PortAudio runtime", "bin/libportaudio*.dll"),
        requirement("SQLite runtime", "bin/libsqlite3*.dll"),
        requirement(
          "bundled Git",
          "git/cmd/git.exe",
          "git/bin/git.exe",
          "bin/git.exe"
        ),
        requirement(
          "Git HTTPS transport",
          "git/mingw64/libexec/git-core/git-remote-https.exe",
          "git/libexec/git-core/git-remote-https.exe"
        ),
        requirement(
          "Git certificate authorities",
          "git/mingw64/ssl/certs/ca-bundle.crt",
          "git/ssl/certs/ca-bundle.crt"
        ),
        requirement(
          "Codex",
          "libexec/codex.exe",
          "libexec/codex-package/bin/codex.exe"
        ),
        requirement("Claude", "libexec/claude.exe"),
        requirement("whisper.cpp", "libexec/whisper.exe"),
        requirement("OpenSSL", "libexec/openssl.exe"),
      ]
    end

    private def requirement(
      label : String,
      *alternatives : String,
    ) : Requirement
      Requirement.new(label, alternatives.to_a)
    end
  end
end
