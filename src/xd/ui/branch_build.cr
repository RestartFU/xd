require "../version"

module Xd
  module UI
    module BranchBuild
      extend self

      REPOSITORY = "RestartFU/xd"

      record Target,
        url : String,
        ref : String,
        label : String

      def parse(text : String?) : Target?
        return unless text

        trimmed = text.strip
        return if trimmed.empty?

        if trimmed.includes?("github.com/")
          return parse_link(trimmed)
        end
        if trimmed.starts_with?("#")
          return parse_pull_request(
            REPOSITORY,
            trimmed.lchop("#")
          )
        end
        if all_digits?(trimmed)
          return parse_pull_request(REPOSITORY, trimmed)
        end

        parse_branch(REPOSITORY, trimmed)
      end

      def checkout_dir(
        cache_home : String = ENV["XDG_CACHE_HOME"]? ||
          File.join(Path.home.to_s, ".cache"),
      ) : String
        File.join(cache_home, DATA_NAME, "source")
      end

      def command(target : Target, checkout : String) : String
        parent = File.dirname(checkout)
        quoted_checkout = Process.quote_posix(checkout)
        quoted_parent = Process.quote_posix(parent)
        quoted_url = Process.quote_posix(target.url)
        quoted_ref = Process.quote_posix(target.ref)

        <<-SHELL
        set -eu
        checkout=#{quoted_checkout}
        mkdir -p #{quoted_parent}
        [ -d "$checkout/.git" ] || git init -q "$checkout"
        git -C "$checkout" fetch --depth 1 --force #{quoted_url} #{quoted_ref}
        git -C "$checkout" checkout -q --force --detach FETCH_HEAD
        git -C "$checkout" clean -qfdx
        cd "$checkout"
        grep -q -- '--from)' scripts/install.sh || { echo "this branch's installer cannot install a local build; rebase it on master" >&2; exit 1; }
        ./scripts/build.sh --build-arg PROFILE=nightly
        sh scripts/install.sh --from dist
        SHELL
      end

      private def parse_link(text : String) : Target?
        marker = "github.com/"
        start = text.index(marker)
        return unless start

        path = text[(start + marker.bytesize)..]
        query = path.index('?')
        fragment = path.index('#')
        cut = if query && fragment
                Math.min(query, fragment)
              else
                query || fragment
              end
        path = path[0, cut] if cut
        parts = path.split('/')
        return if parts.size < 2

        owner = parts[0]
        repository = parts[1]
        repository = repository.rchop(".git") if repository.ends_with?(".git")
        return unless repo_part?(owner) && repo_part?(repository)

        repo = "#{owner}/#{repository}"
        if parts.size >= 4 && parts[2] == "pull"
          parse_pull_request(repo, parts[3])
        elsif parts.size >= 4 && parts[2] == "tree"
          parse_branch(repo, parts[3..].join('/'))
        end
      end

      private def parse_pull_request(
        repository : String,
        number : String,
      ) : Target?
        return unless all_digits?(number)
        return if number.bytesize > 9

        target(
          repository,
          "pull request ##{number}",
          "refs/pull/#{number}/head"
        )
      end

      private def parse_branch(
        repository : String,
        branch : String,
      ) : Target?
        return unless ref_name?(branch)

        target(
          repository,
          "branch #{branch}",
          "refs/heads/#{branch}"
        )
      end

      private def target(
        repository : String,
        label : String,
        ref : String,
      ) : Target
        shown = repository == REPOSITORY ? label : "#{label} in #{repository}"
        Target.new(
          "https://github.com/#{repository}.git",
          ref,
          shown
        )
      end

      private def ref_name?(name : String) : Bool
        return false if name.empty? || name.bytesize > 200
        return false unless name.each_byte.all? do |byte|
                              ascii_alphanumeric?(byte) ||
                              byte.in?('.'.ord.to_u8, '_'.ord.to_u8, '-'.ord.to_u8, '/'.ord.to_u8)
                            end
        return false if {'-', '/', '.'}.includes?(name[0])
        return false if {'/', '.'}.includes?(name[-1])
        return false if name.includes?("..") || name.includes?("//")

        !name.ends_with?(".lock")
      end

      private def repo_part?(part : String) : Bool
        return false if part.empty? || part.starts_with?('-')

        part.each_byte.all? do |byte|
          ascii_alphanumeric?(byte) ||
            byte.in?('.'.ord.to_u8, '_'.ord.to_u8, '-'.ord.to_u8)
        end
      end

      private def all_digits?(text : String) : Bool
        !text.empty? && text.each_byte.all? { |byte| byte.in?('0'.ord.to_u8..'9'.ord.to_u8) }
      end

      private def ascii_alphanumeric?(byte : UInt8) : Bool
        byte.in?('a'.ord.to_u8..'z'.ord.to_u8) ||
          byte.in?('A'.ord.to_u8..'Z'.ord.to_u8) ||
          byte.in?('0'.ord.to_u8..'9'.ord.to_u8)
      end
    end
  end
end
