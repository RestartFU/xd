require "json"
require "../storage/workflow_state"
require "../workspace/service"

module Xd
  module Daemon
    class Filesystem
      FILE_LIMIT = 1024 * 1024

      class Error < Exception
      end

      def initialize(
        @store : Storage::Store,
        @workspaces : Workspace::Service,
      )
      end

      def list_directory(requested : String?) : Hash(String, JSON::Any)
        path = requested.presence || Path.home.to_s
        names = Dir.children(path)
          .reject(&.starts_with?('.'))
          .select { |name| File.directory?(File.join(path, name)) }
          .sort

        {
          "path"    => JSON::Any.new(path),
          "entries" => json_any(names),
        }
      rescue error : File::Error
        raise Error.new(error.message || "Cannot list that directory")
      end

      def browse(
        chat_id : String,
        action : String,
        relative : String?,
        content : String?,
      ) : Hash(String, JSON::Any)
        path = chat_path(chat_id, relative)

        case action
        when "list"
          browse_list(path)
        when "read"
          browse_read(path)
        when "write"
          unless content
            raise Error.new("file-browse write needs content.")
          end
          browse_write(path, content)
        else
          raise Error.new("No such file-browse action.")
        end
      rescue error : File::Error
        raise Error.new(error.message || "Cannot browse that file")
      end

      def workdir(chat_id : String) : String
        chat = @store.get_chat(chat_id)
        @workspaces.resolve_workdir(chat.folder_id, chat.workdir)
      end

      def chat_path(chat_id : String, relative : String?) : String
        root = File.realpath(workdir(chat_id))
        value = relative || ""
        if Path[value].absolute?
          raise Error.new(
            "File paths must be relative to the working directory."
          )
        end

        candidate = File.expand_path(value, root)
        resolved = File.realpath(candidate)
        unless within?(resolved, root)
          raise Error.new("That file is outside the working directory.")
        end
        resolved
      rescue error : File::Error
        raise Error.new(error.message || "Cannot resolve that file")
      end

      private def browse_list(path : String) : Hash(String, JSON::Any)
        entries = Dir.children(path)
          .reject(&.starts_with?('.'))
          .map do |name|
            JSON::Any.new({
              "name"      => JSON::Any.new(name),
              "directory" => JSON::Any.new(
                File.directory?(File.join(path, name))
              ),
            })
          end
          .sort_by do |entry|
            {
              entry["directory"].as_bool ? 0 : 1,
              entry["name"].as_s,
            }
          end

        {"entries" => JSON::Any.new(entries)}
      end

      private def browse_read(path : String) : Hash(String, JSON::Any)
        info = regular_file(path, "previewed")
        if info.size > FILE_LIMIT
          raise Error.new("Files larger than 1 MB are not previewed.")
        end

        content = File.read(path)
        if content.bytesize > FILE_LIMIT
          raise Error.new("Files larger than 1 MB are not previewed.")
        end
        if content.includes?('\0') || !content.valid_encoding?
          raise Error.new("Binary files cannot be previewed as text.")
        end

        {"content" => JSON::Any.new(content)}
      end

      private def browse_write(
        path : String,
        content : String,
      ) : Hash(String, JSON::Any)
        if content.bytesize > FILE_LIMIT
          raise Error.new("Files larger than 1 MB cannot be saved here.")
        end
        regular_file(path, "edited")
        File.write(path, content)
        {} of String => JSON::Any
      end

      private def regular_file(path : String, verb : String) : File::Info
        info = File.info(path, follow_symlinks: false)
        unless info.type.file?
          raise Error.new("Only regular files can be #{verb}.")
        end
        info
      end

      private def within?(path : String, root : String) : Bool
        path == root || path.starts_with?(root + File::SEPARATOR)
      end

      private def json_any(value) : JSON::Any
        JSON.parse(value.to_json)
      end
    end
  end
end
