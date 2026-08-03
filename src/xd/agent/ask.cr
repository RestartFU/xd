module Xd
  module Agent
    # Structured question embedded in an assistant reply.
    #
    # Both bundled CLIs run non-interactively. A tagged block lets the daemon
    # preserve the reply verbatim while every client renders the same buttons.
    class Ask
      OPEN  = "<ask>"
      CLOSE = "</ask>"

      MAX_OPTIONS = 6

      INSTRUCTIONS = <<-TEXT
        <asking_the_user>
        You cannot prompt for input: you are running non-interactively, and anything you ask arrives as text the user must answer in a new message.

        When a decision is genuinely the user's to make and the answers are a short list, wrap the question so it can be shown as buttons:

        <ask>
        Which approach should I take?
        - Fix the parser
        - Rewrite the parser
        </ask>

        The question goes on the first line and each option on its own line starting with "- ". Give two to six options, each a complete answer rather than a letter. Put the block at the end of your reply.

        When the answer is specific text rather than a short list, put an input marker on its own line instead:

        <ask>
        What branch name should I use?
        <input>
        </ask>

        This shows a text field. An <ask> may contain either two to six options, an <input> marker, or both.

        Ask at most one question per reply. Put exactly one <ask> block at the end of the reply, with no prose after </ask>.

        Use it only when different answers lead to materially different work. Anything you can settle yourself, or find out by looking, is not a question -- decide it and say what you decided.
        </asking_the_user>

        <agent_speech>
        When a hands-free client should read a concise part of the reply aloud, wrap only that part in an exact <speak>...</speak> block. Speech is optional: do not put ordinary prose, code, tool output, status updates, analysis, or questions in the block. Clients may ignore the block when speech is disabled.
        </agent_speech>

        <commit_links>
        When reporting a Git commit, make the hash text a Markdown link to that commit's web URL. Example: [abc1234](https://github.com/owner/repo/commit/abc1234). Use the actual repository URL and hash. Do not report a bare hash when its web URL can be determined from the repository remote.
        </commit_links>

        <issue_links>
        When mentioning a GitHub issue or pull request, make the visible reference a Markdown link to its web URL whenever the repository and number can be determined. Examples: [#35](https://github.com/owner/repo/issues/35) and [PR #12](https://github.com/owner/repo/pull/12). Do not leave a resolvable #number as bare text.
        </issue_links>

        <workspace_reporting>
        If you move the work to another Git worktree, report the new checkout root on its own line as:

        <workspace>/absolute/path/to/checkout</workspace>

        Report it as soon as the new worktree becomes the directory where you are working. Use an absolute path. Do not report ordinary subdirectory changes or emit the tag when the checkout did not change.
        </workspace_reporting>
        TEXT

      record Parsed, ask : Ask, remainder : String

      getter question : String
      getter options : Array(String)
      getter accepts_input : Bool

      def initialize(
        @question : String,
        @options : Array(String),
        @accepts_input : Bool,
      )
      end

      # Returns the last valid block and strips every valid block from prose.
      # Older agents sometimes emit several despite the one-question contract;
      # raw control tags must never reach Markdown rendering.
      def self.parse(text : String) : Parsed?
        blocks = [] of {Int32, Int32, Ask}
        offset = 0

        while open_at = text.byte_index(OPEN, offset)
          if candidate = parse_at(text, open_at)
            close_at = text.byte_index(
              CLOSE,
              open_at + OPEN.bytesize
            ).not_nil!
            finish = close_at + CLOSE.bytesize
            blocks << {open_at, finish, candidate}
            offset = finish
          else
            offset = open_at + 1
          end
        end

        return if blocks.empty?

        segments = [] of String
        at = 0
        blocks.each do |start, finish, _ask|
          segment = text.byte_slice(at, start - at).strip
          segments << segment unless segment.empty?
          at = finish
        end
        segment = text.byte_slice(at, text.bytesize - at).strip
        segments << segment unless segment.empty?
        remainder = segments.join("\n\n")
        Parsed.new(blocks.last[2], remainder)
      end

      # Bytes safe to expose while a UTF-8 reply streams. Holds complete valid
      # blocks, unclosed blocks, and partial opening tags at the tail.
      def self.visible_bytes(text : String) : Int32
        if parsed_at = first_valid_open(text)
          return parsed_at
        end

        offset = 0
        while open_at = text.byte_index(OPEN, offset)
          close_at = text.byte_index(CLOSE, open_at + OPEN.bytesize)
          return open_at unless close_at
          offset = open_at + 1
        end

        length = text.bytesize
        Math.min(OPEN.bytesize - 1, length).downto(1) do |back|
          tail = text.byte_slice(length - back, back)
          prefix = OPEN.byte_slice(0, back)
          return length - back if tail == prefix
        end
        length
      end

      private def self.first_valid_open(text : String) : Int32?
        offset = 0
        while open_at = text.byte_index(OPEN, offset)
          return open_at if parse_at(text, open_at)
          offset = open_at + 1
        end
        nil
      end

      private def self.parse_at(text : String, open_at : Int32) : Ask?
        body_start = open_at + OPEN.bytesize
        close_at = text.byte_index(CLOSE, body_start)
        return unless close_at

        body = text.byte_slice(body_start, close_at - body_start)
        question = [] of String
        options = [] of String
        accepts_input = false

        body.lines.each do |line|
          stripped = line.strip
          next if stripped.empty?

          if stripped == "<input>"
            accepts_input = true
          elsif stripped.starts_with?("- ") || stripped.starts_with?("* ")
            if options.size < MAX_OPTIONS
              options << stripped.byte_slice(2).strip
            end
          elsif options.empty?
            question << stripped
          end
        end

        return if options.size < 2 && !accepts_input

        Ask.new(
          question.empty? ? "Which one?" : question.join(" "),
          options,
          accepts_input
        )
      end
    end
  end
end
