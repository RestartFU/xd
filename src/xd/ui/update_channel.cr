require "json"
require "../version"

module Xd
  module UI
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

      def tag(channel : Channel) : String?
        channel.nightly? ? "nightly" : nil
      end

      def check_url(channel : Channel) : String
        if release = tag(channel)
          "https://api.github.com/repos/#{REPOSITORY}/releases/tags/#{release}"
        else
          "https://api.github.com/repos/#{REPOSITORY}/releases/latest"
        end
      end

      def install_command(channel : Channel) : String
        if release = tag(channel)
          "curl -fsSL https://github.com/#{REPOSITORY}/releases/" \
          "download/#{release}/install.sh | sh"
        else
          "curl -fsSL https://github.com/#{REPOSITORY}/releases/" \
          "latest/download/install.sh | sh -s -- --release"
        end
      end

      def latest_from_reply(
        channel : Channel,
        body : String,
      ) : String?
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
        return false unless latest
        return false if latest.empty?

        if channel.nightly?
          return false if current_commit.empty?

          !latest.starts_with?(current_commit)
        else
          version = latest.starts_with?("v") ? latest.lchop("v") : latest
          version != current_version
        end
      end
    end
  end
end
