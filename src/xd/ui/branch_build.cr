require "../bundle_environment"
require "../version"

module Xd
  module UI
    module BranchBuild
      extend self

      REPOSITORY = "RestartFU/xd"

      record Target, url : String, ref : String, label : String

      def supported? : Bool
        {% if flag?(:linux) && flag?(:x86_64) %}
          true
        {% else %}
          false
        {% end %}
      end

      def parse(text : String?) : Target?
        return unless text
        value = text.strip
        return if value.empty?
        return parse_link(value) if value.includes?("github.com/")
        return parse_pull_request(REPOSITORY, value.lchop('#')) if value.starts_with?('#')
        return parse_pull_request(REPOSITORY, value) if digits?(value)
        return parse_commit(REPOSITORY, value) if commit?(value)
        parse_branch(REPOSITORY, value)
      end

      def checkout_dir : String
        cache = ENV["XDG_CACHE_HOME"]? || File.join(Path.home, ".cache")
        File.join(cache, DATA_NAME, "source")
      end

      def command(target : Target, checkout : String) : String
        git = BundleEnvironment.executable("git") || "git"
        parent = File.dirname(checkout)
        <<-SHELL
        set -eu
        checkout=#{Process.quote_posix(checkout)}
        mkdir -p #{Process.quote_posix(parent)}
        [ -d "$checkout/.git" ] || #{Process.quote_posix(git)} init -q "$checkout"
        #{Process.quote_posix(git)} -C "$checkout" fetch --depth 1 --force #{Process.quote_posix(target.url)} #{Process.quote_posix(target.ref)}
        #{Process.quote_posix(git)} -C "$checkout" checkout -q --force --detach FETCH_HEAD
        #{Process.quote_posix(git)} -C "$checkout" clean -qfdx
        cd "$checkout"
        ./scripts/build.sh --build-arg PROFILE=nightly
        XD_ALLOW_RUNNING_INSTALL=1 sh scripts/install.sh --from dist
        SHELL
      end

      private def parse_link(text : String) : Target?
        start = text.index("github.com/")
        return unless start
        path = text[(start + 11)..]
        cut = [path.index('?'), path.index('#')].compact.min?
        path = path[0, cut] if cut
        parts = path.split('/')
        return if parts.size < 2
        owner = parts[0]
        repo_name = parts[1].rchop(".git")
        return unless repo_part?(owner) && repo_part?(repo_name)
        repo = "#{owner}/#{repo_name}"
        return parse_pull_request(repo, parts[3]) if parts.size >= 4 && parts[2] == "pull"
        return parse_branch(repo, parts[3..].join('/')) if parts.size >= 4 && parts[2] == "tree"
        return parse_commit(repo, parts[3]) if parts.size >= 4 && parts[2] == "commit"
        nil
      end

      private def parse_pull_request(repository : String, number : String) : Target?
        return unless digits?(number) && number.bytesize <= 9
        target(repository, "pull request ##{number}", "refs/pull/#{number}/head")
      end

      private def parse_commit(repository : String, commit : String) : Target?
        return unless commit?(commit)
        target(repository, "commit #{commit[0, Math.min(12, commit.size)]}", commit)
      end

      private def parse_branch(repository : String, branch : String) : Target?
        return unless ref_name?(branch)
        target(repository, "branch #{branch}", "refs/heads/#{branch}")
      end

      private def target(repository : String, label : String, ref : String) : Target
        shown = repository == REPOSITORY ? label : "#{label} in #{repository}"
        Target.new("https://github.com/#{repository}.git", ref, shown)
      end

      private def commit?(value : String) : Bool
        value.bytesize.in?(7..40) && value.each_byte.all? do |byte|
          byte.in?('0'.ord.to_u8..'9'.ord.to_u8) ||
            byte.in?('a'.ord.to_u8..'f'.ord.to_u8) ||
            byte.in?('A'.ord.to_u8..'F'.ord.to_u8)
        end
      end

      private def ref_name?(name : String) : Bool
        return false if name.empty? || name.bytesize > 200
        return false unless name.each_byte.all? do |byte|
                              alphanumeric?(byte) || byte.in?('.'.ord.to_u8, '_'.ord.to_u8, '-'.ord.to_u8, '/'.ord.to_u8)
                            end
        return false if {'-', '/', '.'}.includes?(name[0])
        return false if {'/', '.'}.includes?(name[-1])
        !name.includes?("..") && !name.includes?("//") && !name.ends_with?(".lock")
      end

      private def repo_part?(part : String) : Bool
        !part.empty? && !part.starts_with?('-') && part.each_byte.all? do |byte|
          alphanumeric?(byte) || byte.in?('.'.ord.to_u8, '_'.ord.to_u8, '-'.ord.to_u8)
        end
      end

      private def digits?(text : String) : Bool
        !text.empty? && text.each_byte.all? { |byte| byte.in?('0'.ord.to_u8..'9'.ord.to_u8) }
      end

      private def alphanumeric?(byte : UInt8) : Bool
        byte.in?('a'.ord.to_u8..'z'.ord.to_u8) ||
          byte.in?('A'.ord.to_u8..'Z'.ord.to_u8) ||
          byte.in?('0'.ord.to_u8..'9'.ord.to_u8)
      end
    end
  end
end
