require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/images"

private def with_images(
  & : Xd::Daemon::Images, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-images-#{Random::Secure.hex(12)}"
  )
  old_cache = ENV["XDG_CACHE_HOME"]?
  old_name = ENV["XD_DATA_NAME"]?
  ENV["XDG_CACHE_HOME"] = directory
  ENV["XD_DATA_NAME"] = "xd-images-spec"

  begin
    yield Xd::Daemon::Images.new, directory
  ensure
    if old_cache
      ENV["XDG_CACHE_HOME"] = old_cache
    else
      ENV.delete("XDG_CACHE_HOME")
    end
    if old_name
      ENV["XD_DATA_NAME"] = old_name
    else
      ENV.delete("XD_DATA_NAME")
    end
    FileUtils.rm_r(directory)
  end
end

describe Xd::Daemon::Images do
  it "materializes private PNG files and reads them back" do
    with_images do |images, _directory|
      png = Xd::Daemon::Images::PNG_SIGNATURE + Bytes[1_u8, 2_u8, 3_u8]
      message = images.materialize({
        "attachments" => JSON.parse([{
          "name" => "../../ignored.png",
          "mime" => "image/png",
          "data" => Base64.strict_encode(png),
        }].to_json),
      }, "inspect")
      path = message.match(/\[image: (.+)\]/).not_nil![1]

      File.info(path).permissions.to_i.should eq(0o600)
      File.read(path).to_slice.should eq(png)
      decoded = Base64.decode(images.read(path)["data"].as_s)
      decoded.should eq(png)
    end
  end

  it "rejects invalid uploads and paths outside the private cache" do
    with_images do |images, directory|
      expect_raises(Xd::Daemon::Images::Error, /Only PNG/) do
        images.materialize({
          "attachments" => JSON.parse([{
            "mime" => "image/jpeg",
            "data" => Base64.strict_encode("not png"),
          }].to_json),
        }, "")
      end

      outside = File.join(directory, "outside.png")
      File.write(outside, Xd::Daemon::Images::PNG_SIGNATURE)
      expect_raises(Xd::Daemon::Images::Error, /not a remote paste/) do
        images.read(outside)
      end
    end
  end
end
