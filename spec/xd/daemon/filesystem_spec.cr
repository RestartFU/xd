require "../../spec_helper"
require "file_utils"
require "random/secure"
require "../../../src/xd/daemon/filesystem"

private def with_filesystem(
  & : Xd::Daemon::Filesystem, String, String ->
) : Nil
  directory = File.join(
    Dir.tempdir,
    "xd-filesystem-#{Random::Secure.hex(12)}"
  )
  store = Xd::Storage::Store.new(File.join(directory, "chats.db"))
  workspaces = Xd::Workspace::Service.new(
    File.join(directory, "Workspaces"),
    store
  )
  folder_id = workspaces.create_folder(nil, "Project")
  folder = workspaces.find_folder(folder_id)
  chat_id = store.create_chat(folder_id, "Chat", "claude")
  filesystem = Xd::Daemon::Filesystem.new(store, workspaces)

  begin
    yield filesystem, folder, chat_id
  ensure
    store.close
    FileUtils.rm_r(directory)
  end
end

describe Xd::Daemon::Filesystem do
  it "lists visible directories for the daemon-side picker" do
    with_filesystem do |filesystem, folder, _chat_id|
      Dir.mkdir(File.join(folder, "visible"))
      Dir.mkdir(File.join(folder, ".hidden"))
      File.write(File.join(folder, "note.txt"), "text")

      listed = filesystem.list_directory(folder)

      listed["path"].as_s.should eq(folder)
      listed["entries"].as_a.map(&.as_s).should eq(["visible"])
    end
  end

  it "lists, reads, and writes regular files inside the chat workdir" do
    with_filesystem do |filesystem, folder, chat_id|
      Dir.mkdir(File.join(folder, "src"))
      File.write(File.join(folder, "note.txt"), "before\n")

      entries = filesystem.browse(chat_id, "list", "", nil)["entries"].as_a
      entries.map { |entry| entry["name"].as_s }
        .should eq(["src", "note.txt"])
      filesystem.browse(chat_id, "read", "note.txt", nil)["content"]
        .as_s.should eq("before\n")
      filesystem.browse(chat_id, "write", "note.txt", "after\n")
      File.read(File.join(folder, "note.txt")).should eq("after\n")
    end
  end

  it "rejects traversal, symlink escapes, binary files, and oversized files" do
    with_filesystem do |filesystem, folder, chat_id|
      outside = File.join(File.dirname(folder), "outside.txt")
      File.write(outside, "secret")
      File.symlink(outside, File.join(folder, "link.txt"))
      File.write(File.join(folder, "binary"), "a\0b")
      File.write(File.join(folder, "large"), "x" * (1024 * 1024 + 1))

      expect_raises(Xd::Daemon::Filesystem::Error, /outside/) do
        filesystem.browse(chat_id, "read", "../outside.txt", nil)
      end
      expect_raises(Xd::Daemon::Filesystem::Error, /outside/) do
        filesystem.browse(chat_id, "read", "link.txt", nil)
      end
      expect_raises(Xd::Daemon::Filesystem::Error, /Binary/) do
        filesystem.browse(chat_id, "read", "binary", nil)
      end
      expect_raises(Xd::Daemon::Filesystem::Error, /larger than 1 MB/) do
        filesystem.browse(chat_id, "read", "large", nil)
      end
    end
  end
end
