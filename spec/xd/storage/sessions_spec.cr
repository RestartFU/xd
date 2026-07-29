require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/sessions"

private def with_session_store(& : Xd::Storage::Store ->) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-sessions-#{Random::Secure.hex(12)}",
    "chats.db"
  )
  store = Xd::Storage::Store.new(path)

  begin
    yield store
  ensure
    store.close
    FileUtils.rm_r(Path[path].dirname)
  end
end

describe Xd::Storage::Store do
  it "keeps resumable sessions per backend and replaces ids" do
    with_session_store do |store|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.set_session_id(chat_id, "claude", "first")
      store.set_session_id(chat_id, "claude", "second")
      store.set_session_id(chat_id, "codex", "codex-session")

      store.get_session_id(chat_id, "claude").should eq("second")
      store.get_session_id(chat_id, "codex").should eq("codex-session")
      store.get_session_id(chat_id, "missing").should be_nil

      store.set_session_id(chat_id, "claude", nil)
      store.get_session_id(chat_id, "claude").should be_nil
      store.get_session_id(chat_id, "codex").should eq("codex-session")
    end
  end

  it "tracks how far each backend has seen and resets forgotten sessions" do
    with_session_store do |store|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.append_message(chat_id, "user", "who are you")
      store.append_message(chat_id, "assistant", "Claude here")
      after_claude = store.last_message_id(chat_id)
      store.set_session_id(chat_id, "claude", "session")
      store.set_last_seen(chat_id, "claude", after_claude)

      store.append_message(chat_id, "user", "and you?")
      store.append_message(chat_id, "assistant", "Codex here")

      store.get_last_seen(chat_id, "claude").should eq(after_claude)
      store.get_last_seen(chat_id, "codex").should eq(0)
      store.list_messages_since(chat_id, after_claude)
        .map(&.content).should eq(["and you?", "Codex here"])

      store.set_session_id(chat_id, "claude", nil)
      store.get_last_seen(chat_id, "claude").should eq(0)
    end
  end

  it "binds context usage to backend session and model" do
    with_session_store do |store|
      chat_id = store.create_chat(
        "folder",
        "Chat",
        "claude",
        "claude-opus"
      )
      store.set_session_id(chat_id, "claude", "session")
      store.set_context_usage(
        chat_id,
        "claude",
        "claude-opus",
        48_750_u64,
        1_000_000_u64
      )

      usage = store.get_context_usage(
        chat_id,
        "claude",
        "claude-opus"
      ).not_nil!
      usage.used.should eq(48_750)
      usage.window.should eq(1_000_000)
      store.get_context_usage(
        chat_id,
        "claude",
        "claude-haiku"
      ).should be_nil

      store.set_session_id(chat_id, "claude", nil)
      store.get_context_usage(
        chat_id,
        "claude",
        "claude-opus"
      ).should be_nil
    end
  end
end
