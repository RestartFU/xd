require "../../spec_helper"
require "../../../src/xd/ui/download_progress"

describe Xd::UI::DownloadProgress do
  it "reads the newest percentage out of a redrawn meter" do
    chunk = "\r#####            7.8%\r######           8.7%"
    reading = Xd::UI::DownloadProgress.read(chunk)
    reading.percent.should eq(8)
    reading.text.should be_empty
  end

  it "ignores the marks curl draws before it knows a size" do
    reading = Xd::UI::DownloadProgress.read("\r#=#=#      \r##O#-#     ")
    reading.percent.should be_nil
    reading.text.should be_empty
  end

  it "keeps what went wrong out of the meter" do
    reading = Xd::UI::DownloadProgress.read(
      "\r####            4.2%\rinstall: cannot replace /home/x/.local/opt/xd.\n"
    )
    reading.percent.should eq(4)
    reading.text.should eq(
      "install: cannot replace /home/x/.local/opt/xd.\n"
    )
  end

  it "rounds down and never reports past a full download" do
    Xd::UI::DownloadProgress.read("\r  99.9%").percent.should eq(99)
    Xd::UI::DownloadProgress.read("\r####### 100.0%\n").percent.should eq(100)
  end

  it "reports nothing for a chunk that carries no meter" do
    reading = Xd::UI::DownloadProgress.read("curl: (28) Operation timed out\n")
    reading.percent.should be_nil
    reading.text.should eq("curl: (28) Operation timed out\n")
  end
end
