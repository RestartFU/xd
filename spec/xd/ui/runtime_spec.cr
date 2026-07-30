require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/ui/runtime"

describe Xd::UI::Runtime do
  it "rejects an accepting endpoint that cannot answer ping" do
    directory = File.join(
      Dir.tempdir,
      "xd-runtime-readiness-#{Random::Secure.hex(12)}"
    )
    old_data = ENV["XDG_DATA_HOME"]?
    old_name = ENV["XD_DATA_NAME"]?
    ENV["XDG_DATA_HOME"] = directory
    ENV["XD_DATA_NAME"] = "readiness"
    socket_path = Xd::AppPaths.local_socket
    listener = UNIXServer.new(socket_path)
    clients = [] of UNIXSocket
    lock = Mutex.new
    spawn do
      while client = listener.accept?
        lock.synchronize { clients << client }
      end
    rescue IO::Error
    end

    started = Time.instant
    expect_raises(IO::Error, /already running/) do
      Xd::UI::Runtime.new(20.milliseconds)
    end
    (Time.instant - started).should be < 1.second
  ensure
    listener.try(&.close)
    lock.try do |mutex|
      mutex.synchronize { clients.try(&.each(&.close)) }
    end
    if old_data
      ENV["XDG_DATA_HOME"] = old_data
    else
      ENV.delete("XDG_DATA_HOME")
    end
    if old_name
      ENV["XD_DATA_NAME"] = old_name
    else
      ENV.delete("XD_DATA_NAME")
    end
    FileUtils.rm_r(directory) if directory && Dir.exists?(directory)
  end
end
