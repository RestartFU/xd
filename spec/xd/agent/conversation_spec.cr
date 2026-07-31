require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/agent/conversation"

private def with_conversation_store(
  & : Xd::Storage::Store, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-conversation-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  chat_id = store.create_chat("folder", "Chat", "claude")

  begin
    yield store, chat_id
  ensure
    store.close
    FileUtils.rm_r(directory)
  end
end

describe Xd::Agent::Conversation do
  it "retells unseen conversation without repeating the current prompt" do
    with_conversation_store do |store, chat_id|
      store.append_message(chat_id, "user", "first")
      store.append_message(chat_id, "assistant", "first answer")
      store.append_message(chat_id, "tool", "Read src/main.cr")
      store.append_message(chat_id, "user", "current")

      handover = Xd::Agent::Conversation.handover(store, chat_id, 0_i64)
      handover.should_not be_nil
      handover.not_nil!.should contain("User: first")
      handover.not_nil!.should_not contain("Read src/main.cr")
      handover.not_nil!.should_not contain("current")
    end
  end

  it "does not create an empty handover from events or tools" do
    with_conversation_store do |store, chat_id|
      store.append_message(chat_id, "event", "Switched to GPT-5.6 Terra")
      store.append_message(chat_id, "user", "current")

      Xd::Agent::Conversation.handover(store, chat_id, 0_i64)
        .should be_nil
    end
  end

  it "does not spend the handover budget on hidden tool rows" do
    with_conversation_store do |store, chat_id|
      store.append_message(chat_id, "user", "first")
      store.append_message(chat_id, "assistant", "first answer")
      store.append_message(chat_id, "tool", "x" * 20_000)
      store.append_message(chat_id, "user", "current")

      handover = Xd::Agent::Conversation.handover(store, chat_id, 0_i64)
      handover.should_not be_nil
      handover.not_nil!.should contain("User: first")
      handover.not_nil!.should contain("Assistant: first answer")
      handover.not_nil!.should_not contain("x" * 100)
    end
  end

  it "keeps slash commands first when joining a handover" do
    joined = Xd::Agent::Conversation.join("earlier", "/review now")
    joined.should eq("/review now\n\nearlier")

    Xd::Agent::Conversation.join("earlier", "continue")
      .should eq("earlier\n\ncontinue")
  end

  it "uses only a shortened first line for chat titles" do
    prompt = "#{"x" * 60}\nignored"
    Xd::Agent::Conversation.title(prompt)
      .should eq("#{"x" * 48}…")
    Xd::Agent::Conversation.title(" \nrest").should be_nil
  end
end
