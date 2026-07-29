require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/chats"

private def with_chat_store(& : Xd::Storage::Store, Proc(Int64) ->) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-chats-#{Random::Secure.hex(12)}",
    "chats.db"
  )
  now = 1_000_000_i64
  clock = -> { now }
  store = Xd::Storage::Store.new(path, clock)

  begin
    yield store, -> { now += 1_000_000 }
  ensure
    store.close
    FileUtils.rm_r(Path[path].dirname)
  end
end

describe Xd::Storage::Store do
  it "creates, reads, lists, and renames chats by stable folder id" do
    with_chat_store do |store, tick|
      first = store.create_chat(
        "folder-a",
        "Rate limiting",
        "claude"
      )
      tick.call
      store.create_chat("folder-b", "Elsewhere", "codex")

      chats = store.list_chats("folder-a")
      chats.size.should eq(1)
      chats[0].id.should eq(first)
      chats[0].title.should eq("Rate limiting")
      chats[0].backend.should eq("claude")

      store.set_chat_title(first, "Renamed")
      store.get_chat(first).title.should eq("Renamed")
      store.list_chats("folder-b").size.should eq(1)
    end
  end

  it "shares daemon turn ownership through SQLite" do
    with_chat_store do |store, _tick|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.get_chat(chat_id).daemon_working.should be_false

      store.set_daemon_working(chat_id, true)
      store.get_chat(chat_id).daemon_working.should be_true

      store.clear_daemon_working
      store.get_chat(chat_id).daemon_working.should be_false
      expect_raises(Xd::Storage::NotFoundError) do
        store.set_daemon_working("missing", true)
      end
    end
  end

  it "inherits the complete last changed agent configuration" do
    with_chat_store do |store, _tick|
      changed = store.create_chat(
        "folder-a",
        "Changed",
        "claude",
        "claude-opus",
        "medium"
      )
      before = store.create_chat(
        "folder-b",
        "Before",
        "codex",
        "gpt-default",
        "low"
      )
      store.get_chat(before).backend.should eq("codex")

      store.set_backend(changed, "codex")
      store.set_model(changed, "gpt-5.6")
      store.set_effort(changed, "xhigh")
      store.set_access(changed, "full")
      store.set_plan(changed, true)

      after = store.create_chat(
        "folder-b",
        "After",
        "claude",
        "claude-haiku",
        "low"
      )
      chat = store.get_chat(after)
      chat.backend.should eq("codex")
      chat.model.should eq("gpt-5.6")
      chat.effort.should eq("xhigh")
      chat.access.should eq("full")
      chat.plan.should be_true
    end
  end

  it "reads legacy single-message queue rows" do
    Xd::Storage.queue_from_column(nil).should be_empty
    Xd::Storage.queue_from_column("").should be_empty
    Xd::Storage.queue_from_column("one").should eq(["one"])
    Xd::Storage.queue_from_column(%(["one","","two"])).should eq(
      ["one", "two"]
    )
  end
end
