require "digest/sha256"

module Xd
  module Agent
    module Environment
      extend self

      REWRITTEN = {
        "PATH",
        "XDG_DATA_DIRS",
        "LANG",
        "LC_ALL",
        "LOCPATH",
        "LOCALE_ARCHIVE",
        "GIO_EXTRA_MODULES",
        "GIO_MODULE_DIR",
        "GSETTINGS_SCHEMA_DIR",
        "GSETTINGS_BACKEND",
        "GDK_PIXBUF_MODULE_FILE",
        "GTK_IM_MODULE",
        "GTK_IM_MODULE_FILE",
        "GTK_MODULES",
        "GTK_PATH",
        "GTK_THEME",
        "GTK_DATA_PREFIX",
        "GTK_EXE_PREFIX",
        "GSK_RENDERER",
        "XCURSOR_PATH",
        "FONTCONFIG_FILE",
        "FONTCONFIG_PATH",
        "XKB_CONFIG_ROOT",
        "XLOCALEDIR",
        "SSL_CERT_FILE",
        "OPENSSL_CONF",
        "OPENSSL_MODULES",
        "GIT_EXEC_PATH",
        "GIT_TEMPLATE_DIR",
        "GIT_SSL_CAINFO",
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "LIBGL_DRIVERS_PATH",
        "LIBGL_ALWAYS_SOFTWARE",
      }

      BUNDLE_ONLY = {
        "XD_AGENT_SECRETS_FILE",
      }

      def host(source : Hash(String, String) = ENV.to_h) : Hash(String, String)
        environment = source.dup
        REWRITTEN.each do |name|
          marker = "XD_HOST_#{name}"
          next unless environment.has_key?(marker)

          value = environment[marker]
          if value.empty?
            environment.delete(name)
          else
            environment[name] = value
          end
          environment.delete(marker)
        end
        BUNDLE_ONLY.each { |name| environment.delete(name) }
        environment
      end

      def allowed_names(
        environment : Hash(String, String),
        secret_names : Array(String),
      ) : Array(String)?
        return nil if secret_names.empty?

        environment.keys.select do |name|
          !looks_secret?(name) || secret_names.includes?(name)
        end.sort
      end

      def pool_key(
        executable : String,
        environment : Hash(String, String),
      ) : String
        digest = Digest::SHA256.new
        digest.update(executable)
        environment.to_a
          .sort_by(&.[0])
          .each do |name, value|
            digest.update(Bytes[0_u8])
            digest.update(name)
            digest.update("=")
            digest.update(value)
          end
        digest.final.hexstring
      end

      private def looks_secret?(name : String) : Bool
        lowered = name.downcase
        lowered.includes?("key") ||
          lowered.includes?("secret") ||
          lowered.includes?("token")
      end
    end
  end
end
