require "../../spec_helper"
require "base64"
require "file_utils"
require "random/secure"
require "../../../src/xd/ui/image_attachment"

private def with_attachment_file(data : Bytes, & : String ->) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-image-attachment-#{Random::Secure.hex(12)}"
  )
  File.write(path, data)
  begin
    yield path
  ensure
    File.delete?(path)
  end
end

describe Xd::UI::ImageAttachment do
  it "normalizes a file and returns copied preview pixels" do
    png = Base64.decode(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwC" \
      "AAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
    )

    with_attachment_file(png) do |path|
      prepared = Xd::UI::ImageAttachment.prepare_file(path)
      prepared.png[0, 8].should eq(Bytes[
        0x89_u8, 0x50_u8, 0x4e_u8, 0x47_u8,
        0x0d_u8, 0x0a_u8, 0x1a_u8, 0x0a_u8,
      ])
      prepared.preview.width.should eq(1)
      prepared.preview.height.should eq(1)
      prepared.preview.data.empty?.should be_false
      texture = Xd::UI::ImageAttachment.texture(prepared.preview)
      texture.width.should eq(1)
      texture.height.should eq(1)
    end
  end

  it "stops reading once source byte budget is exceeded" do
    with_attachment_file(Bytes.new(65, 0_u8)) do |path|
      expect_raises(
        Xd::UI::ImageAttachment::Error,
        /source image must be 10 MiB or smaller/
      ) do
        Xd::UI::ImageAttachment.prepare_file(path, 64)
      end
    end
  end
end
