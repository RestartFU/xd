require "../spec_helper"
require "file_utils"
require "random/secure"
require "../../src/xd/native_bundle"

private def materialize_bundle(
  root : String,
  platform : Xd::NativeBundle::Platform,
) : Nil
  Xd::NativeBundle.requirements(platform).each do |requirement|
    path = requirement.alternatives.first.gsub('*', "proof")
    target = File.join(root, path)
    Dir.mkdir_p(File.dirname(target))
    File.write(target, requirement.label)
  end
end

describe Xd::NativeBundle do
  {% for platform in %w(MacOS Windows) %}
    it "requires a complete {{platform.id.downcase}} runtime payload" do
      directory = File.join(
        Dir.tempdir,
        "xd-native-bundle-#{Random::Secure.hex(12)}"
      )
      platform = Xd::NativeBundle::Platform::{{platform.id}}

      begin
        missing = Xd::NativeBundle.validate(directory, platform)
        missing.should contain("Crystal executable")
        missing.should contain("bundled Git")
        missing.should contain("PortAudio runtime")
        missing.should contain("Codex")
        missing.should contain("Adwaita icon theme")
        missing.should contain("chat symbolic icon")
        missing.should contain("microphone symbolic icon")
        missing.should contain("accounts symbolic icon")
        missing.should contain("SVG pixbuf loader")

        materialize_bundle(directory, platform)
        Xd::NativeBundle.validate(directory, platform).should be_empty
        windows_root = directory.gsub('/', '\\')
        Xd::NativeBundle.validate(windows_root, platform).should be_empty
      ensure
        FileUtils.rm_r(directory) if Dir.exists?(directory)
      end
    end
  {% end %}

  it "accepts the current MSYS2 SVG loader name" do
    directory = File.join(
      Dir.tempdir,
      "xd-native-bundle-#{Random::Secure.hex(12)}"
    )

    begin
      materialize_bundle(directory, Xd::NativeBundle::Platform::Windows)
      synthetic = Dir.glob(
        File.join(directory, "lib/gdk-pixbuf-2.0/*/loaders/*svg*.dll")
      ).first
      File.delete(synthetic)
      File.write(
        File.join(File.dirname(synthetic), "pixbufloader_svg.dll"),
        "SVG pixbuf loader"
      )

      Xd::NativeBundle.validate(
        directory,
        Xd::NativeBundle::Platform::Windows
      ).should be_empty
    ensure
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end
end
