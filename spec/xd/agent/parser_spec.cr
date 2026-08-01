require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/parser"

private def replay_agent_fixture(
  backend : Xd::Agent::Backend,
  name : String,
) : Array(Xd::Agent::Event)
  parser = Xd::Agent::Parser.new(backend)
  events = [] of Xd::Agent::Event
  File.each_line(File.join("tests", "fixtures", name)) do |line|
    events.concat(parser.feed_line(line.chomp))
  end
  events
end

private def event_text(
  events : Array(Xd::Agent::Event),
  type : Xd::Agent::EventType,
) : String
  events.select { |event| event.type == type }
    .compact_map(&.text)
    .join
end

describe Xd::Agent::Parser do
  it "bounds Claude slash commands before publishing them" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    commands = 250.times.map { |index| JSON::Any.new("/command-#{index}") }
      .to_a
    line = {
      "type"           => JSON::Any.new("system"),
      "subtype"        => JSON::Any.new("init"),
      "session_id"     => JSON::Any.new("bounded-commands"),
      "slash_commands" => JSON::Any.new(commands),
    }.to_json

    event = parser.feed_line(line).find(&.type.commands?).not_nil!
    event.commands.not_nil!.size.should eq(Xd::Agent::Event::MAX_COMMANDS)
    event.commands.not_nil!.last.should eq("command-199")
  end

  it "streams Claude text once and reports session, commands, and usage" do
    events = replay_agent_fixture(
      Xd::Agent::Catalog::CLAUDE,
      "claude-stream.jsonl"
    )

    event_text(events, Xd::Agent::EventType::TextDelta)
      .should eq("hello from hy")
    session = events.find do |event|
      event.type.session_started?
    end.not_nil!
    session.session_id.should eq(
      "653dbf2a-6521-4412-9ac9-81b4d94160e7"
    )
    commands = events.find { |event| event.type.commands? }
      .not_nil!
      .commands
      .not_nil!
    commands.should contain("simplify")
    commands.should contain("review")

    usage = events.find { |event| event.type.usage? }.not_nil!
    usage.context_used.should eq(21_335_u64)
    usage.context_window.should eq(1_000_000_u64)
    events.count(&.type.result?).should eq(1)
    events.count(&.type.error?).should eq(0)
  end

  it "reports captured Claude tools without duplicating finished messages" do
    events = replay_agent_fixture(
      Xd::Agent::Catalog::CLAUDE,
      "claude-tool-use.jsonl"
    )
    events.count(&.type.tool_use?).should be >= 1
    events.count(&.type.result?).should eq(1)
    events.count(&.type.error?).should eq(0)
  end

  it "reads exact Codex context from the bounded rollout tail" do
    directory = File.join(
      Dir.tempdir,
      "xd-parser-codex-#{Random::Secure.hex(12)}"
    )
    sessions = File.join(directory, "sessions")
    rollout = File.join(
      sessions,
      "rollout-019f9b16-df5f-7182-bdc6-1cce26148979.jsonl"
    )
    old_home = ENV["CODEX_HOME"]?

    begin
      Dir.mkdir_p(sessions)
      File.write(
        rollout,
        %({"payload":{"type":"token_count","info":{) \
        %("last_token_usage":{"total_tokens":15555},) \
        %("model_context_window":258400}}}) + "\n"
      )
      ENV["CODEX_HOME"] = directory
      events = replay_agent_fixture(
        Xd::Agent::Catalog::CODEX,
        "codex-exec.jsonl"
      )

      event_text(events, Xd::Agent::EventType::TextDelta)
        .should eq("hello from hy")
      session = events.find(&.type.session_started?).not_nil!
      session.session_id.should eq(
        "019f9b16-df5f-7182-bdc6-1cce26148979"
      )
      usage = events.find(&.type.usage?).not_nil!
      usage.context_used.should eq(15_555_u64)
      usage.context_window.should eq(258_400_u64)
      events.count(&.type.tool_use?).should eq(1)
      events.count(&.type.result?).should eq(1)
      events.count(&.type.error?).should eq(0)
    ensure
      if old_home
        ENV["CODEX_HOME"] = old_home
      else
        ENV.delete("CODEX_HOME")
      end
      FileUtils.rm_r(directory) if Dir.exists?(directory)
    end
  end

  it "survives garbage and future events" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    events = [
      "",
      "not json",
      %({"type":"something_new","payload":42}),
      "[1,2,3]",
      %({"type":"stream_event","event":{"type":"content_block_delta",) \
      %("delta":{"type":"text_delta","text":"still here"}}}),
    ].flat_map { |line| parser.feed_line(line) }

    event_text(events, Xd::Agent::EventType::TextDelta)
      .should eq("still here")
  end

  it "emits streamed tool arguments in order" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    lines = [
      {
        type:  "stream_event",
        event: {
          type:          "content_block_start",
          index:         0,
          content_block: {
            type:  "tool_use",
            name:  "Read",
            input: {} of String => String,
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 0,
          delta: {
            type:         "input_json_delta",
            partial_json: %({"file_path":),
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 0,
          delta: {
            type:         "input_json_delta",
            partial_json: %("src/main.c"}),
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {type: "content_block_stop", index: 0},
      }.to_json,
      {
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 1,
          delta: {type: "text_delta", text: "done"},
        },
      }.to_json,
    ]
    events = lines.flat_map { |line| parser.feed_line(line) }

    events.count(&.type.tool_use?).should eq(1)
    events.find(&.type.tool_use?).not_nil!.text
      .should eq("Read  src/main.c")
    event_text(events, Xd::Agent::EventType::TextDelta).should eq("done")
  end

  it "bounds streamed Claude tool arguments before parsing" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    parser.feed_line({
      type:  "stream_event",
      event: {
        type:          "content_block_start",
        index:         0,
        content_block: {
          type: "tool_use",
          id:   "large-write",
          name: "Write",
        },
      },
    }.to_json)
    fragment = "x" * 64 * 1024
    33.times do
      parser.feed_line({
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 0,
          delta: {
            type:         "input_json_delta",
            partial_json: fragment,
          },
        },
      }.to_json)
    end
    parser.feed_line({
      type:  "stream_event",
      event: {type: "content_block_stop", index: 0},
    }.to_json)

    events = parser.feed_line({
      type:    "user",
      message: {
        content: [{type: "tool_result", tool_use_id: "large-write"}],
      },
    }.to_json)
    events.size.should eq(1)
    events.first.text.should eq("file_change")
  end

  it "defers Claude file changes until their tool result" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    request = [
      {
        type:  "stream_event",
        event: {
          type:          "content_block_start",
          index:         0,
          content_block: {
            type:  "tool_use",
            id:    "toolu_edit",
            name:  "Edit",
            input: {} of String => String,
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 0,
          delta: {
            type:         "input_json_delta",
            partial_json: %({"file_path":"src/main.c"}),
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {type: "content_block_stop", index: 0},
      }.to_json,
    ]
    before = request.flat_map { |line| parser.feed_line(line) }
    before.count(&.type.tool_use?).should eq(0)

    after = parser.feed_line({
      type:    "user",
      message: {
        content: [
          {
            type:        "tool_result",
            tool_use_id: "toolu_edit",
            content:     "updated",
          },
        ],
      },
    }.to_json)
    after.count(&.type.tool_use?).should eq(1)
    after.first.text.should eq("file_change  src/main.c")
  end

  it "emits Claude inline edit diffs without a repository" do
    parser = Xd::Agent::Parser.new(Xd::Agent::Catalog::CLAUDE)
    request = [
      {
        type:  "stream_event",
        event: {
          type:          "content_block_start",
          index:         0,
          content_block: {
            type:  "tool_use",
            id:    "toolu_inline_edit",
            name:  "Edit",
            input: {} of String => String,
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {
          type:  "content_block_delta",
          index: 0,
          delta: {
            type:         "input_json_delta",
            partial_json: {
              file_path:  "src/main.c",
              old_string: "puts(\"old\")\n",
              new_string: "puts(\"new\")\n",
            }.to_json,
          },
        },
      }.to_json,
      {
        type:  "stream_event",
        event: {type: "content_block_stop", index: 0},
      }.to_json,
    ]
    request.each { |line| parser.feed_line(line) }

    events = parser.feed_line({
      type:    "user",
      message: {
        content: [
          {
            type:        "tool_result",
            tool_use_id: "toolu_inline_edit",
            content:     "updated",
          },
        ],
      },
    }.to_json)

    events.size.should eq(1)
    patch = events.first.text.not_nil!
    patch.should start_with(
      "file_change\ndiff --git a/src/main.c b/src/main.c"
    )
    patch.should contain("-puts(\"old\")")
    patch.should contain("+puts(\"new\")")
  end
end
