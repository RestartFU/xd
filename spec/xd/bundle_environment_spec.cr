require "../spec_helper"
require "file_utils"
require "random/secure"
require "../../src/xd/bundle_environment"

private def preserving_environment(& : ->) : Nil
  previous = ENV.to_h
  begin
    yield
  ensure
    ENV.to_h.each_key { |name| ENV.delete(name) }
    previous.each { |name, value| ENV[name] = value }
  end
end

private def executable_file(path : String) : Nil
  Dir.mkdir_p(File.dirname(path))
  File.write(path, "")
  File.chmod(path, 0o700)
end

describe Xd::BundleEnvironment do
  it "locates flat and macOS application bundle roots" do
    directory = File.join(
      Dir.tempdir,
      "xd-bundle-root-#{Random::Secure.hex(12)}"
    )
    flat = File.join(directory, "flat")
    Dir.mkdir_p(File.join(flat, "libexec"))
    macos = File.join(directory, "xd.app", "Contents", "MacOS")
    resources = File.join(directory, "xd.app", "Contents", "Resources")
    Dir.mkdir_p(macos)
    Dir.mkdir_p(File.join(resources, "share", "glib-2.0"))

    begin
      Xd::BundleEnvironment.locate(
        File.join(flat, "bin", "xd")
      ).should eq(flat)
      Xd::BundleEnvironment.locate(
        File.join(macos, "xd")
      ).should eq(resources)
      Xd::BundleEnvironment.locate(
        File.join(directory, "plain", "xd")
      ).should be_nil
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "prepares native runtime data and bundled tools in-process" do
    directory = File.join(
      Dir.tempdir,
      "xd-bundle-environment-#{Random::Secure.hex(12)}"
    )
    root = File.join(directory, "xd.app", "Contents", "Resources")
    cache = File.join(directory, "cache")
    bin = File.join(root, "bin")
    git_bin = File.join(root, "git", "bin")
    git_exec = File.join(root, "git", "libexec", "git-core")
    templates = File.join(root, "git", "share", "git-core", "templates")
    modules = File.join(root, "lib", "gio", "modules")
    schemas = File.join(root, "share", "glib-2.0", "schemas")
    fonts = File.join(root, "etc", "fonts")
    certificates = File.join(root, "git", "ssl", "certs")
    openssl_config = File.join(root, "etc", "ssl", "openssl.cnf")
    [bin, git_bin, git_exec, templates, modules, schemas, fonts, certificates]
      .each { |path| Dir.mkdir_p(path) }
    codex = File.join(root, "libexec", "codex-package", "bin", "codex")
    executable_file(codex)
    executable_file(File.join(root, "libexec", "openssl"))
    executable_file(File.join(git_bin, "git"))
    File.write(
      File.join(root, "etc", "gdk-pixbuf-loaders.cache.in"),
      "\"@BUNDLE@/lib/pixbuf-loader\"\n"
    )
    File.write(
      File.join(root, "etc", "fonts.conf.in"),
      "<dir>@BUNDLE@/share/fonts</dir>\n"
    )
    File.write(File.join(certificates, "ca-bundle.crt"), "test certificate")
    Dir.mkdir_p(File.dirname(openssl_config))
    File.write(openssl_config, "openssl_conf = default_conf")

    begin
      preserving_environment do
        ENV["PATH"] = "/host/bin"
        ENV["XDG_CACHE_HOME"] = cache
        ENV["GTK_PATH"] = "/host/gtk"
        {
          "XDG_DATA_DIRS",
          "GIO_MODULE_DIR",
          "GIO_EXTRA_MODULES",
          "GSETTINGS_SCHEMA_DIR",
          "GSETTINGS_BACKEND",
          "GTK_DATA_PREFIX",
          "GTK_EXE_PREFIX",
          "GTK_IM_MODULE",
          "GDK_PIXBUF_MODULE_FILE",
          "FONTCONFIG_FILE",
          "FONTCONFIG_PATH",
          "GIT_EXEC_PATH",
          "GIT_TEMPLATE_DIR",
          "GIT_SSL_CAINFO",
          "SSL_CERT_FILE",
          "XD_OPENSSL",
          "OPENSSL_CONF",
          "XD_HOST_GTK_PATH",
        }.each { |name| ENV.delete(name) }

        Xd::BundleEnvironment.prepare(root)
        Xd::BundleEnvironment.prepare(root)

        path = ENV["PATH"].split(':')
        path.first.should eq(git_bin)
        path.count(git_bin).should eq(1)
        path.count(bin).should eq(1)
        ENV["XDG_DATA_DIRS"].should eq(File.join(root, "share"))
        ENV["GIO_MODULE_DIR"].should eq(modules)
        ENV["GIO_EXTRA_MODULES"].should eq(modules)
        ENV["GSETTINGS_SCHEMA_DIR"].should eq(schemas)
        ENV["GSETTINGS_BACKEND"].should eq("keyfile")
        ENV["GTK_PATH"].should eq(root)
        ENV["XD_HOST_GTK_PATH"].should eq("/host/gtk")
        ENV["GIT_EXEC_PATH"].should eq(git_exec)
        ENV["GIT_TEMPLATE_DIR"].should eq(templates)
        ENV["GIT_SSL_CAINFO"].should eq(
          File.join(certificates, "ca-bundle.crt")
        )
        ENV["SSL_CERT_FILE"].should eq(
          File.join(certificates, "ca-bundle.crt")
        )
        Xd::BundleEnvironment.executable("codex", root).should eq(
          codex
        )
        Xd::BundleEnvironment.executable("git", root).should eq(
          File.join(git_bin, "git")
        )
        ENV["XD_OPENSSL"].should eq(
          File.join(root, "libexec", "openssl")
        )
        ENV["OPENSSL_CONF"].should eq(openssl_config)

        pixbuf = ENV["GDK_PIXBUF_MODULE_FILE"]
        fonts_file = ENV["FONTCONFIG_FILE"]
        File.read(pixbuf).should contain(
          "#{root}/lib/pixbuf-loader"
        )
        File.read(fonts_file).should contain(
          "#{root}/share/fonts"
        )
        ENV["FONTCONFIG_PATH"].should eq(fonts)
      end
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "uses the current PortableGit TLS layout" do
    directory = File.join(
      Dir.tempdir,
      "xd-portable-git-environment-#{Random::Secure.hex(12)}"
    )
    root = File.join(directory, "bundle")
    certificates = File.join(
      root,
      "git",
      "mingw64",
      "etc",
      "ssl",
      "certs",
      "ca-bundle.crt"
    )
    openssl_config = File.join(
      root,
      "git",
      "mingw64",
      "etc",
      "ssl",
      "openssl.cnf"
    )
    Dir.mkdir_p(File.dirname(certificates))
    File.write(certificates, "portable certificate")
    File.write(openssl_config, "openssl_conf = portable")

    begin
      preserving_environment do
        ENV.delete("GIT_SSL_CAINFO")
        ENV.delete("SSL_CERT_FILE")
        ENV.delete("OPENSSL_CONF")

        Xd::BundleEnvironment.prepare(root)

        ENV["GIT_SSL_CAINFO"].should eq(certificates)
        ENV["SSL_CERT_FILE"].should eq(certificates)
        ENV["OPENSSL_CONF"].should eq(openssl_config)
      end
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
