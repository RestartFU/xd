require "json"
require "./version"

module Xd
  # Where a build looks for its own updates.
  #
  # Not UI: the daemon uses this to update itself when a client asks, so it
  # cannot live behind GTK.
  module UpdateChannel
    extend self

    enum Channel
      Release
      Nightly
    end

    REPOSITORY = "RestartFU/xd"

    def current : Channel
      BUILD_PROFILE == "nightly" ? Channel::Nightly : Channel::Release
    end

    def check_url(channel : Channel) : String
      if channel.nightly?
        "https://api.github.com/repos/#{REPOSITORY}/releases/tags/nightly"
      else
        "https://api.github.com/repos/#{REPOSITORY}/releases/latest"
      end
    end

    def installer_url(channel : Channel) : String
      base = "https://github.com/#{REPOSITORY}/releases"
      channel.nightly? ? "#{base}/download/nightly/install.sh" : "#{base}/latest/download/install.sh"
    end

    def install_command(channel : Channel, curl : String) : String
      release = channel.release? ? " -s -- --release" : ""
      quoted = Process.quote_posix(curl)
      "#{quoted} -fsSL #{installer_url(channel)} | " \
      "XD_ALLOW_RUNNING_INSTALL=1 XD_CURL=#{quoted} sh#{release}"
    end

    def latest_from_reply(channel : Channel, body : String) : String?
      release = JSON.parse(body).as_h?
      return unless release
      key = channel.nightly? ? "target_commitish" : "tag_name"
      release[key]?.try(&.as_s?)
    rescue JSON::ParseException
      nil
    end

    def newer?(
      channel : Channel,
      latest : String?,
      current_commit : String = BUILD_COMMIT,
      current_version : String = VERSION,
    ) : Bool
      return false unless latest && !latest.empty?
      if channel.nightly?
        !current_commit.empty? && !latest.starts_with?(current_commit)
      else
        latest.lchop("v") != current_version
      end
    end
  end
end
