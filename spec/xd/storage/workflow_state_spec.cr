require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/storage/workflow_state"

private def with_workflow_store(& : Xd::Storage::Store ->) : Nil
  path = File.join(
    Dir.tempdir,
    "xd-workflow-state-#{Random::Secure.hex(12)}",
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
  it "keeps, edits, promotes, removes, and consumes every queued message" do
    with_workflow_store do |store|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.queue_append(chat_id, "first thing")
      store.queue_append(chat_id, "second thing")
      store.queue_append(chat_id, "third thing")
      store.get_chat(chat_id).queue.should eq(
        ["first thing", "second thing", "third thing"]
      )

      store.queue_promote(chat_id, 2)
      store.get_chat(chat_id).queue.should eq(
        ["third thing", "first thing", "second thing"]
      )
      store.queue_replace(
        chat_id,
        1,
        "first thing",
        "edited first thing"
      )
      expect_raises(Xd::Storage::ConflictError) do
        store.queue_replace(chat_id, 1, "first thing", "stale edit")
      end

      store.queue_remove(chat_id, 2)
      store.queue_take_first(chat_id).should eq("third thing")
      store.queue_take_first(chat_id).should eq("edited first thing")
      store.queue_take_first(chat_id).should be_nil
      store.get_chat(chat_id).queue.should be_empty
    end
  end

  it "preserves queues while atomically taking restart markers" do
    with_workflow_store do |store|
      first = store.create_chat("folder", "First", "claude")
      second = store.create_chat("folder", "Second", "codex")
      store.queue_append(first, "user queued this")

      store.mark_resumes([first, second])
      store.take_resumes.sort.should eq([first, second].sort)
      store.get_chat(first).queue.should eq(["user queued this"])
      store.take_resumes.should be_empty

      expect_raises(Xd::Storage::NotFoundError) do
        store.mark_resumes([first, "missing"])
      end
      store.take_resumes.should be_empty
    end
  end

  it "locks workspace selection after the first message" do
    with_workflow_store do |store|
      chat_id = store.create_chat("folder", "Chat", "claude")
      store.set_new_worktree(chat_id, true)
      store.get_chat(chat_id).new_worktree.should be_true

      store.use_existing_worktree(
        chat_id,
        "/tmp/existing-worktree",
        "/tmp/original-checkout"
      )
      chat = store.get_chat(chat_id)
      chat.new_worktree.should be_false
      chat.workdir.should eq("/tmp/existing-worktree")
      chat.original_workdir.should eq("/tmp/original-checkout")

      store.append_message(chat_id, "user", "start")
      expect_raises(Xd::Storage::ConflictError) do
        store.use_existing_worktree(
          chat_id,
          "/tmp/another-worktree",
          "/tmp/original-checkout"
        )
      end
    end
  end

  it "preserves and restores the first checkout" do
    with_workflow_store do |store|
      chat_id = store.create_chat(
        "folder",
        "Chat",
        "claude",
        workdir: "/tmp/original-checkout"
      )
      store.switch_workdir(
        chat_id,
        "/tmp/first-worktree",
        "/tmp/original-checkout"
      )
      store.switch_workdir(
        chat_id,
        "/tmp/second-worktree",
        "/tmp/first-worktree"
      )

      chat = store.get_chat(chat_id)
      chat.workdir.should eq("/tmp/second-worktree")
      chat.original_workdir.should eq("/tmp/original-checkout")

      store.restore_workdir(chat_id, chat.original_workdir.not_nil!)
      restored = store.get_chat(chat_id)
      restored.workdir.should eq("/tmp/original-checkout")
      restored.original_workdir.should be_nil
    end
  end
end
