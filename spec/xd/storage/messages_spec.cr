require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/messages"

private def with_message_store(
  & : Xd::Storage::Store, Proc(Int64) ->
) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-messages-#{Random::Secure.hex(12)}",
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
  it "round-trips, updates, and removes messages" do
    with_message_store do |store, tick|
      chat_id = store.create_chat("folder", "Chat", "claude")
      user_id = store.append_message(
        chat_id,
        "user",
        "how do I add a rate limiter?"
      )
      tick.call
      assistant_id = store.append_message(
        chat_id,
        "assistant",
        "Use a token bucket.",
        %({"type":"result"}),
        "Claude Opus · High"
      )

      messages = store.list_messages(chat_id)
      messages.map(&.id).should eq([user_id, assistant_id])
      messages[0].role.should eq("user")
      messages[1].raw_json.should eq(%({"type":"result"}))
      messages[1].label.should eq("Claude Opus · High")

      store.update_message(assistant_id, "Use a bounded token bucket.")
      store.list_messages(chat_id)[1].content.should contain("bounded")
      store.delete_message(assistant_id)
      store.list_messages(chat_id).map(&.id).should eq([user_id])

      expect_raises(Xd::Storage::NotFoundError) do
        store.update_message(assistant_id, "gone")
      end
    end
  end

  it "bounds recent rows, omits raw events, and filters by message id" do
    with_message_store do |store, tick|
      chat_id = store.create_chat("folder", "Chat", "claude")
      ids = 5.times.map do |index|
        tick.call
        store.append_message(
          chat_id,
          index.even? ? "user" : "assistant",
          "message-#{index}",
          %({"large":"backend event"})
        )
      end.to_a

      recent = store.list_recent_messages(chat_id, 2)
      recent.total.should eq(5)
      recent.messages.map(&.content).should eq(["message-3", "message-4"])
      recent.messages.each(&.raw_json.should(be_nil))

      through = store.list_recent_messages_through(chat_id, ids[2], 5)
      through.total.should eq(3)
      through.messages.map(&.content).should eq(
        ["message-0", "message-1", "message-2"]
      )
      store.list_messages_since(chat_id, ids[2]).map(&.content).should eq(
        ["message-3", "message-4"]
      )
      store.last_message_id(chat_id).should eq(ids.last)
    end
  end

  it "orders chats only by their latest user instruction" do
    with_message_store do |store, tick|
      first = store.create_chat("folder", "First", "claude")
      tick.call
      second = store.create_chat("folder", "Second", "claude")
      store.list_chats("folder").first.id.should eq(second)

      tick.call
      store.append_message(first, "user", "work here")
      store.list_chats("folder").first.id.should eq(first)

      tick.call
      store.append_message(second, "assistant", "finished")
      store.set_chat_title(second, "Renamed")
      store.list_chats("folder").first.id.should eq(first)

      tick.call
      store.append_message(second, "user", "work here now")
      store.list_chats("folder").first.id.should eq(second)
    end
  end

  it "searches full text and cascades chat deletion" do
    with_message_store do |store, _tick|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.append_message(
        chat_id,
        "user",
        "the websocket reconnect loop is wrong"
      )
      store.append_message(chat_id, "assistant", "add exponential backoff")

      hits = store.search("websocket", 10)
      hits.size.should eq(1)
      hits[0].content.should contain("websocket")
      store.list_folder_ids.should eq(["folder"])

      store.delete_chat(chat_id)
      store.list_messages(chat_id).should be_empty
      store.search("websocket", 10).should be_empty
    end
  end
end
