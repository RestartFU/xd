require "json"
require "./types"

module Xd
  module Agent
    PLAN_INSTRUCTIONS =
      "<plan_mode>\n" \
      "You are planning, not implementing. Produce a plan detailed enough " \
      "that someone else could carry it out without having to make decisions.\n\n" \
      "Explore freely: read and search files, inspect configuration and types, " \
      "run tests, builds and dry runs. Anything that tells you how things " \
      "actually are makes the plan better.\n\n" \
      "Do not carry the work out: no editing or writing files, no patches, " \
      "migrations or codegen, no formatters that rewrite, and no commands whose " \
      "purpose is to do the work rather than to understand it.\n\n" \
      "If asked to go ahead and do something, plan that instead. Plan mode ends " \
      "when the user leaves it, not because a message sounds like an " \
      "instruction.\n\n" \
      "Write the plan as Markdown: \"##\" headings for the parts of the work, " \
      "\"-\" for lists, \"1.\" for steps that happen in order, and fenced code " \
      "blocks for commands, paths and snippets. It is read in a window that " \
      "renders it, so a wall of prose is harder to follow than the same plan with " \
      "its structure showing.\n" \
      "</plan_mode>"

    COMMIT_INSTRUCTIONS =
      "<commit_attribution>\n" \
      "When you create a Git commit, add this trailer to the commit message unless " \
      "the user specifically asks you not to:\n\n" \
      "Co-authored-by: Codex <codex@openai.com>\n" \
      "</commit_attribution>"

    class Backend
      getter id : String
      getter display_name : String
      getter program : String
      getter icon_name : String
      getter transport : Transport
      getter models : Array(Model)
      getter default_model : String

      def initialize(
        @id,
        @display_name,
        @program,
        @icon_name,
        @transport,
        @models,
        @default_model,
      )
      end

      def model_label(model_id : String?) : String
        selected = model_id && !model_id.empty? ? model_id : @default_model
        @models.find { |model| model.id == selected }
          .try(&.display_name) || selected
      end

      def context_window(model_id : String?) : UInt64
        selected = model_id && !model_id.empty? ? model_id : @default_model
        if @id == "codex"
          if dynamic = codex_context_window(selected)
            return dynamic
          end
        end
        @models.find { |model| model.id == selected }
          .try(&.context_window) || 0_u64
      end

      def default_effort : Effort
        case @id
        when "codex"
          path = File.join(Path.home, ".codex", "config.toml")
          effort_from_config(path, "model_reasoning_effort")
        when "claude"
          path = File.join(Path.home, ".claude", "settings.json")
          effort_from_config(path, "effortLevel")
        else
          Effort::High
        end
      end

      def efforts : Array(Effort)
        values = [
          Effort::Low,
          Effort::Medium,
          Effort::High,
          Effort::XHigh,
          Effort::Max,
        ]
        values << Effort::Ultra if @id == "codex"
        values
      end

      def supports_effort?(effort : Effort) : Bool
        efforts.includes?(effort)
      end

      def build_argv(spec : RunSpec) : Array(String)
        case @id
        when "claude" then claude_argv(spec)
        when "codex"  then [@program, "app-server", "--listen", "stdio://"]
        else
          raise ArgumentError.new("Unknown backend: #{@id}")
        end
      end

      def developer_instructions(spec : RunSpec) : String?
        return spec.system_prompt unless @id == "codex"

        sections = [] of String
        sections << PLAN_INSTRUCTIONS if spec.access.plan?
        sections << COMMIT_INSTRUCTIONS
        if prompt = spec.system_prompt
          sections << prompt unless prompt.empty?
        end
        sections.join("\n\n")
      end

      def sandbox_policy(access : Access) : String
        case access
        when Access::Edit then "workspaceWrite"
        when Access::Full then "dangerFullAccess"
        else                   "readOnly"
        end
      end

      private def claude_argv(spec : RunSpec) : Array(String)
        arguments = [@program]
        if session = spec.resume_session_id
          arguments.concat(["--resume", session])
        end
        arguments.concat([
          "-p", spec.prompt,
          "--output-format", "stream-json",
          "--verbose",
          "--include-partial-messages",
        ])
        if model = spec.model
          arguments.concat(["--model", model])
        end
        if prompt = spec.system_prompt
          arguments.concat(["--append-system-prompt", prompt])
        end
        arguments.concat(["--effort", spec.effort.wire_name])
        arguments.concat([
          "--permission-mode",
          case spec.access
          when Access::Plan then "plan"
          when Access::Edit then "acceptEdits"
          when Access::Full then "bypassPermissions"
          else                   "manual"
          end,
        ])
        arguments
      end

      private def effort_from_config(path : String, key : String) : Effort
        return Effort::High unless File.file?(path)

        pattern = Regex.new(
          %("#{Regex.escape(key)}"?\\s*[:=]\\s*"([a-zA-Z]+)")
        )
        match = pattern.match(File.read(path))
        Effort.from_wire(match.try(&.[1].downcase))
      rescue File::Error
        Effort::High
      end

      private def codex_context_window(model_id : String) : UInt64?
        home = ENV["CODEX_HOME"]? || File.join(Path.home, ".codex")
        path = File.join(home, "models_cache.json")
        root = JSON.parse(File.read(path)).as_h?
        models = root.try(&.["models"]?.try(&.as_a?))
        models.try do |items|
          items.each do |item|
            fields = item.as_h?
            next unless fields
            next unless fields["slug"]?.try(&.as_s?) == model_id
            value = fields["context_window"]?.try(&.as_i64?)
            return value.to_u64 if value && value > 0
          end
        end
        nil
      rescue JSON::ParseException | File::Error
        nil
      end
    end

    module Catalog
      extend self

      CLAUDE = Backend.new(
        "claude",
        "Claude Code",
        "claude",
        "xd-backend-claude",
        Transport::Exec,
        [
          Model.new("claude-opus-5", "Claude Opus 5", 0_u64),
          Model.new("claude-fable-5", "Claude Fable 5", 0_u64),
          Model.new("claude-sonnet-5", "Claude Sonnet 5", 0_u64),
          Model.new("claude-haiku-4-5", "Claude Haiku 4.5", 0_u64),
          Model.new("claude-opus-4-8", "Claude Opus 4.8", 0_u64),
        ],
        "claude-opus-5"
      )

      CODEX = Backend.new(
        "codex",
        "Codex",
        "codex",
        "xd-backend-codex-symbolic",
        Transport::CodexAppServer,
        [
          Model.new("gpt-5.6-sol", "GPT-5.6 Sol", 272_000_u64),
          Model.new("gpt-5.6-luna", "GPT-5.6 Luna", 272_000_u64),
          Model.new("gpt-5.6-terra", "GPT-5.6 Terra", 272_000_u64),
          Model.new("gpt-5.5", "GPT-5.5", 272_000_u64),
          Model.new(
            "gpt-5.3-codex-spark",
            "GPT-5.3 Codex Spark",
            128_000_u64
          ),
        ],
        "gpt-5.6-sol"
      )

      ALL = [CLAUDE, CODEX]

      def all : Array(Backend)
        ALL
      end

      def lookup(id : String?) : Backend?
        return nil unless id
        ALL.find { |backend| backend.id == id }
      end
    end
  end
end
