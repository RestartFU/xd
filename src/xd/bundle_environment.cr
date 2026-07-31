require "./version"

module Xd
  # Resolves runtime tools from one relocatable native bundle layout.
  #
  # Linux also has a launcher because its private ELF loader needs arguments.
  # macOS and Windows start the native binary directly, so they need the same
  # environment prepared in-process before any daemon-owned tool is launched.
  module BundleEnvironment
    extend self

    HOST_NAMES = {
      "XDG_DATA_DIRS",
      "LANG",
      "LC_ALL",
      "LOCPATH",
      "LOCALE_ARCHIVE",
      "GIO_EXTRA_MODULES",
      "GTK_IM_MODULE",
      "GTK_PATH",
      "GTK_THEME",
    }

    def prepare(root : String? = locate) : Nil
      return unless root

      remember_host_environment
      prepare_runtime(root)
      prepend_path(File.join(root, "bin"))
      prepare_git(root)
      if openssl = executable("openssl", root)
        ENV["XD_OPENSSL"] ||= openssl
      end
      if openssl_config = first_file(
           File.join(root, "etc", "ssl", "openssl.cnf"),
           File.join(root, "git", "ssl", "openssl.cnf"),
           File.join(root, "git", "mingw64", "etc", "ssl", "openssl.cnf"),
           File.join(root, "git", "mingw64", "ssl", "openssl.cnf")
         )
        ENV["OPENSSL_CONF"] ||= openssl_config
      end
    end

    def executable(name : String, root : String? = locate) : String?
      return unless root

      filename = platform_filename(name)
      [
        File.join(root, "libexec", filename),
        File.join(root, "libexec", "codex-package", "bin", filename),
        File.join(root, "bin", filename),
        File.join(root, "git", "cmd", filename),
        File.join(root, "git", "bin", filename),
        File.join(root, "git", "mingw64", "bin", filename),
      ].each do |candidate|
        return candidate if executable_file?(candidate)
      end
      nil
    end

    def locate(executable : String? = Process.executable_path) : String?
      return unless executable

      directory = File.dirname(File.expand_path(executable))
      parent = File.dirname(directory)
      if File.basename(directory) == "MacOS"
        resources = File.join(parent, "Resources")
        return resources if bundle_directory?(resources)
      end
      parent if bundle_directory?(parent)
    rescue File::Error
      nil
    end

    private def bundle_directory?(root : String) : Bool
      File.directory?(File.join(root, "libexec")) ||
        File.directory?(File.join(root, "git")) ||
        File.directory?(File.join(root, "share", "glib-2.0"))
    end

    private def remember_host_environment : Nil
      HOST_NAMES.each do |name|
        marker = "XD_HOST_#{name}"
        ENV[marker] = ENV[name]? || "" unless ENV.has_key?(marker)
      end
    end

    private def prepare_runtime(root : String) : Nil
      force = native_direct_bundle?(root)
      share = File.join(root, "share")
      modules = File.join(root, "lib", "gio", "modules")
      schemas = File.join(share, "glib-2.0", "schemas")
      if File.directory?(share)
        set_runtime("XDG_DATA_DIRS", share, force)
        set_runtime("GTK_DATA_PREFIX", root, force)
        set_runtime("GTK_EXE_PREFIX", root, force)
        set_runtime("GTK_PATH", root, force)
      end
      if File.directory?(modules)
        set_runtime("GIO_MODULE_DIR", modules, force)
        set_runtime("GIO_EXTRA_MODULES", modules, force)
      end
      if File.directory?(schemas)
        set_runtime("GSETTINGS_SCHEMA_DIR", schemas, force)
        ENV["GSETTINGS_BACKEND"] ||= "keyfile"
      end
      ENV["GTK_IM_MODULE"] ||= "gtk-im-context-simple"

      pixbuf = first_file(
        File.join(
          root,
          "lib",
          "gdk-pixbuf-2.0",
          "2.10.0",
          "loaders.cache"
        ),
        File.join(root, "etc", "gdk-pixbuf-loaders.cache"),
        File.join(root, "etc", "loaders.cache")
      )
      unless pixbuf
        pixbuf = expand_template(
          root,
          first_file(
            File.join(root, "etc", "gdk-pixbuf-loaders.cache.in"),
            File.join(root, "etc", "loaders.cache.in")
          ),
          "gdk-pixbuf-loaders.cache"
        )
      end
      set_runtime("GDK_PIXBUF_MODULE_FILE", pixbuf, force) if pixbuf

      fonts = first_file(File.join(root, "etc", "fonts.conf"))
      fonts ||= expand_template(
        root,
        first_file(File.join(root, "etc", "fonts.conf.in")),
        "fonts.conf"
      )
      if fonts
        set_runtime("FONTCONFIG_FILE", fonts, force)
        font_path = File.join(root, "etc", "fonts")
        set_runtime("FONTCONFIG_PATH", font_path, force) if File.directory?(font_path)
      end
    end

    private def native_direct_bundle?(root : String) : Bool
      {% if flag?(:win32) %}
        true
      {% else %}
        File.basename(root) == "Resources" &&
          File.basename(File.dirname(root)) == "Contents"
      {% end %}
    end

    private def set_runtime(
      name : String,
      value : String,
      force : Bool,
    ) : Nil
      ENV[name] = value if force || !ENV.has_key?(name)
    end

    private def expand_template(
      root : String,
      template : String?,
      output_name : String,
    ) : String?
      return unless template

      contents = File.read(template).gsub(
        "@BUNDLE@",
        root.gsub('\\', '/')
      )
      directory = File.join(cache_home, DATA_NAME)
      Dir.mkdir_p(directory, 0o700)
      output = File.join(directory, output_name)
      File.open(output, "w", perm: 0o600) do |file|
        file << contents
      end
      output
    rescue File::Error
      nil
    end

    private def cache_home : String
      if configured = ENV["XDG_CACHE_HOME"]?
        return configured unless configured.empty?
      end

      {% if flag?(:win32) %}
        ENV["LOCALAPPDATA"]? ||
          File.join(Path.home, "AppData", "Local")
      {% elsif flag?(:darwin) %}
        File.join(Path.home, "Library", "Caches")
      {% else %}
        File.join(Path.home, ".cache")
      {% end %}
    end

    private def prepare_git(root : String) : Nil
      bundled = File.join(root, "bin")
      portable = File.join(root, "git")
      if File.directory?(portable)
        bundled = {% if flag?(:win32) %}
                    File.join(portable, "cmd")
                  {% else %}
                    File.join(portable, "bin")
                  {% end %}
      end
      prepend_path(bundled)

      exec_path = first_directory(
        File.join(root, "libexec", "git-core"),
        File.join(portable, "libexec", "git-core"),
        File.join(portable, "mingw64", "libexec", "git-core")
      )
      ENV["GIT_EXEC_PATH"] = exec_path if exec_path

      templates = first_directory(
        File.join(root, "share", "git-core", "templates"),
        File.join(portable, "share", "git-core", "templates"),
        File.join(
          portable,
          "mingw64",
          "share",
          "git-core",
          "templates"
        )
      )
      ENV["GIT_TEMPLATE_DIR"] = templates if templates

      certificates = first_file(
        File.join(root, "etc", "ssl", "certs", "ca-certificates.crt"),
        File.join(portable, "ssl", "certs", "ca-bundle.crt"),
        File.join(
          portable,
          "mingw64",
          "etc",
          "ssl",
          "certs",
          "ca-bundle.crt"
        ),
        File.join(
          portable,
          "mingw64",
          "ssl",
          "certs",
          "ca-bundle.crt"
        )
      )
      if certificates
        ENV["GIT_SSL_CAINFO"] ||= certificates
        ENV["SSL_CERT_FILE"] ||= certificates
      end
    end

    private def first_directory(*candidates : String) : String?
      candidates.find { |candidate| File.directory?(candidate) }
    end

    private def first_file(*candidates : String) : String?
      candidates.find { |candidate| File.file?(candidate) }
    end

    private def executable_file?(path : String) : Bool
      info = File.info?(path)
      return false unless info.try(&.type.file?)

      {% if flag?(:win32) %}
        true
      {% else %}
        info.not_nil!.permissions.owner_execute?
      {% end %}
    end

    private def prepend_path(directory : String) : Nil
      return unless File.directory?(directory)

      separator = {% if flag?(:win32) %}
                    ';'
                  {% else %}
                    ':'
                  {% end %}
      current = ENV["PATH"]? || ""
      parts = current.split(separator)
      return if parts.includes?(directory)

      ENV["PATH"] = current.empty? ? directory : "#{directory}#{separator}#{current}"
    end

    private def platform_filename(name : String) : String
      {% if flag?(:win32) %}
        name.ends_with?(".exe") ? name : "#{name}.exe"
      {% else %}
        name
      {% end %}
    end
  end
end
