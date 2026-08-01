require "../../spec_helper"
require "../../../src/xd/agent/codex_protocol"

private def sent_json(lines : Array(String), index : Int32) : JSON::Any
  JSON.parse(lines[index])
end

private def initialize_codex_protocol(
  lines : Array(String),
) : Xd::Agent::CodexProtocol
  protocol = Xd::Agent::CodexProtocol.new(
    Xd::Agent::Catalog::CODEX,
    "test-version",
    ->(line : String) { lines << line }
  )
  protocol.initialize_client
  protocol
end

describe Xd::Agent::CodexProtocol do
  it "opens a thread, streams one turn, and records live usage" do
    lines = [] of String
    events = [] of Xd::Agent::Event
    finished = [] of Tuple(Bool, String?)
    protocol = initialize_codex_protocol(lines)
    turn = protocol.start_turn(
      Xd::Agent::RunSpec.new(
        "hello",
        model: "gpt-5.6-sol",
        workdir: "/tmp",
        effort: Xd::Agent::Effort::Ultra,
        access: Xd::Agent::Access::Edit
      ),
      nil,
      ->(event : Xd::Agent::Event) { events << event },
      ->(ok : Bool, message : String?) { finished << {ok, message} }
    )

    sent_json(lines, 0)["method"].as_s.should eq("initialize")
    sent_json(lines, 0)["params"]["clientInfo"]["version"].as_s
      .should eq("test-version")

    protocol.receive_line({
      "id"     => 1,
      "result" => {} of String => String,
    }.to_json)
    protocol.ready.should be_true
    sent_json(lines, 1)["method"].as_s.should eq("initialized")
    sent_json(lines, 2)["method"].as_s.should eq("thread/start")

    protocol.receive_line({
      "id"     => 2,
      "result" => {"thread" => {"id" => "thread-1"}},
    }.to_json)
    turn.thread_id.should eq("thread-1")
    events.last.type.session_started?.should be_true
    sent_json(lines, 3)["method"].as_s.should eq("turn/start")
    start = sent_json(lines, 3)["params"]
    start["effort"].as_s.should eq("ultra")
    start["sandboxPolicy"]["type"].as_s.should eq("workspaceWrite")
    start["sandboxPolicy"]["writableRoots"][0].as_s.should eq("/tmp")
    start["sandboxPolicy"]["networkAccess"].as_bool.should be_false
    start["input"].as_a.map(&.["type"].as_s).should eq(["text"])

    protocol.receive_line({
      "id"     => 3,
      "result" => {"turn" => {"id" => "turn-1"}},
    }.to_json)
    turn.turn_id.should eq("turn-1")

    protocol.receive_line({
      "method" => "item/agentMessage/delta",
      "params" => {
        "threadId" => "thread-1",
        "itemId"   => "message-1",
        "delta"    => "hello",
      },
    }.to_json)
    protocol.receive_line({
      "method" => "item/completed",
      "params" => {
        "threadId" => "thread-1",
        "item"     => {
          "id"   => "message-1",
          "type" => "agentMessage",
          "text" => "hello",
        },
      },
    }.to_json)
    protocol.receive_line({
      "method" => "turn/diff/updated",
      "params" => {
        "threadId" => "thread-1",
        "turnId"   => "turn-1",
        "diff"     => "diff --git a/hello.txt b/hello.txt\n+hello\n",
      },
    }.to_json)
    protocol.receive_line({
      "method" => "item/started",
      "params" => {
        "threadId" => "thread-1",
        "item"     => {
          "id"      => "command-1",
          "type"    => "commandExecution",
          "command" => "pwd",
        },
      },
    }.to_json)
    protocol.receive_line({
      "method" => "item/completed",
      "params" => {
        "threadId" => "thread-1",
        "item"     => {
          "id"      => "change-1",
          "type"    => "fileChange",
          "changes" => [
            {
              "path" => "hello.txt",
              "kind" => "add",
              "diff" => "hello\n",
            },
          ],
        },
      },
    }.to_json)
    protocol.receive_line({
      "method" => "thread/tokenUsage/updated",
      "params" => {
        "threadId"   => "thread-1",
        "tokenUsage" => {
          "last"               => {"totalTokens" => 456},
          "modelContextWindow" => 272_000,
        },
      },
    }.to_json)
    protocol.receive_line({
      "method" => "turn/completed",
      "params" => {
        "threadId" => "thread-1",
        "turn"     => {"status" => "completed"},
      },
    }.to_json)

    events.select(&.type.text_delta?).compact_map(&.text).join
      .should eq("hello")
    events.select(&.type.tool_use?).compact_map(&.text)
      .should eq([
        "$ pwd",
        "file_change\n" \
        "diff --git a/hello.txt b/hello.txt\n" \
        "new file mode 100644\n" \
        "--- /dev/null\n" \
        "+++ b/hello.txt\n" \
        "@@ -0,0 +1,1 @@\n" \
        "+hello",
      ])
    usage = events.find(&.type.usage?).not_nil!
    usage.context_used.should eq(456_u64)
    usage.context_window.should eq(272_000_u64)
    events.count(&.type.result?).should eq(1)
    finished.should eq([{true, nil}])
  end

  it "emits the native turn diff when no file-change item arrives" do
    lines = [] of String
    events = [] of Xd::Agent::Event
    finished = [] of Tuple(Bool, String?)
    protocol = initialize_codex_protocol(lines)
    protocol.receive_line({
      "id"     => 1,
      "result" => {} of String => String,
    }.to_json)
    protocol.start_turn(
      Xd::Agent::RunSpec.new("create a file", workdir: "/tmp"),
      nil,
      ->(event : Xd::Agent::Event) { events << event },
      ->(ok : Bool, message : String?) { finished << {ok, message} }
    )
    protocol.receive_line({
      "id"     => 2,
      "result" => {"thread" => {"id" => "thread-native-diff"}},
    }.to_json)
    protocol.receive_line({
      "id"     => 3,
      "result" => {"turn" => {"id" => "turn-native-diff"}},
    }.to_json)

    patch = "diff --git a/generated.txt b/generated.txt\n" \
            "new file mode 100644\n" \
            "--- /dev/null\n" \
            "+++ b/generated.txt\n" \
            "@@ -0,0 +1 @@\n" \
            "+generated\n"
    protocol.receive_line({
      "method" => "turn/diff/updated",
      "params" => {
        "threadId" => "thread-native-diff",
        "turnId"   => "turn-native-diff",
        "diff"     => patch,
      },
    }.to_json)
    protocol.receive_line({
      "method" => "turn/completed",
      "params" => {
        "threadId" => "thread-native-diff",
        "turn"     => {"status" => "completed"},
      },
    }.to_json)

    events.select(&.type.tool_use?).compact_map(&.text)
      .should eq(["file_change\n#{patch.rstrip}"])
    events.map(&.type).last.result?.should be_true
    finished.should eq([{true, nil}])
  end

  it "resumes with private environment policy and interrupts by ids" do
    lines = [] of String
    finished = [] of Tuple(Bool, String?)
    protocol = initialize_codex_protocol(lines)
    protocol.receive_line({
      "id"     => 1,
      "result" => {} of String => String,
    }.to_json)

    turn = protocol.start_turn(
      Xd::Agent::RunSpec.new(
        "continue",
        model: "gpt-5.6-sol",
        system_prompt: "folder rules",
        resume_session_id: "thread-old",
        workdir: "/workspace",
        access: Xd::Agent::Access::Full
      ),
      ["PATH", "CUSTOM_TOKEN"],
      ->(_event : Xd::Agent::Event) { },
      ->(ok : Bool, message : String?) { finished << {ok, message} }
    )
    opened = sent_json(lines, 2)
    opened["method"].as_s.should eq("thread/resume")
    opened["params"]["threadId"].as_s.should eq("thread-old")
    opened["params"]["sandbox"].as_s.should eq("danger-full-access")
    opened["params"]["developerInstructions"].as_s
      .should contain("folder rules")
    opened["params"]["config"]["shell_environment_policy"]["include_only"]
      .as_a
      .map(&.as_s)
      .should eq(["PATH", "CUSTOM_TOKEN"])

    protocol.receive_line({
      "id"     => 2,
      "result" => {"thread" => {"id" => "thread-old"}},
    }.to_json)
    protocol.receive_line({
      "id"     => 3,
      "result" => {"turn" => {"id" => "turn-live"}},
    }.to_json)
    protocol.cancel(turn)

    interrupted = sent_json(lines, 4)
    interrupted["method"].as_s.should eq("turn/interrupt")
    interrupted["params"]["threadId"].as_s.should eq("thread-old")
    interrupted["params"]["turnId"].as_s.should eq("turn-live")

    protocol.receive_line({
      "method" => "turn/completed",
      "params" => {
        "threadId" => "thread-old",
        "turn"     => {"status" => "interrupted"},
      },
    }.to_json)
    finished.should eq([{true, nil}])
  end

  it "denies approvals, rejects unknown requests, and fails waiting turns" do
    lines = [] of String
    finished = [] of Tuple(Bool, String?)
    protocol = initialize_codex_protocol(lines)
    protocol.start_turn(
      Xd::Agent::RunSpec.new("waiting"),
      nil,
      ->(_event : Xd::Agent::Event) { },
      ->(ok : Bool, message : String?) { finished << {ok, message} }
    )

    protocol.receive_line({
      "id"     => 71,
      "method" => "item/fileChange/requestApproval",
      "params" => {} of String => String,
    }.to_json)
    sent_json(lines, 1)["result"]["decision"].as_s.should eq("cancel")

    protocol.receive_line({
      "id"     => 72,
      "method" => "future/serverRequest",
      "params" => {} of String => String,
    }.to_json)
    sent_json(lines, 2)["error"]["code"].as_i64.should eq(-32601)

    protocol.fail("server stopped")
    protocol.failed.should be_true
    finished.should eq([{false, "server stopped"}])
  end
end
