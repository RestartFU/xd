require "json"

module Xd
  module Agent
    # Builds display-only unified patches from tool payloads. This path needs
    # no repository, Git executable, filesystem read, or post-tool snapshot.
    module ToolDiff
      extend self

      PREFIX = "file_change\n"
      LIMIT  = 256 * 1024

      def build(
        name : String?,
        input : Hash(String, JSON::Any)?,
      ) : String?
        return unless name && input

        patch = case name.downcase
                when "file_change", "filechange"
                  codex(input)
                when "edit", "edit_file"
                  edit(input)
                when "write", "write_file"
                  write(input)
                when "multiedit"
                  multi_edit(input)
                else
                  nil
                end
        return unless patch

        patch = patch.rstrip
        return if patch.empty?
        PREFIX + truncate(patch)
      end

      private def codex(input : Hash(String, JSON::Any)) : String?
        changes = input["changes"]?.try(&.as_a?)
        return unless changes

        patches = changes.compact_map do |node|
          change = node.as_h?
          next unless change
          path = string?(change, "path") || string?(change, "filePath")
          next unless path
          diff = string?(change, "diff") || ""
          kind_node = change["kind"]?
          kind = kind_node.try(&.as_s?)
          kind_object = kind_node.try(&.as_h?)
          kind ||= kind_object.try(&.keys.first?)
          kind = kind.try(&.downcase) || "update"

          case kind
          when "add"
            new_file(path, diff)
          when "delete"
            deleted_file(path, diff)
          else
            move = kind_object.try do |object|
              string?(object, "move_path") ||
                string?(object, "movePath") ||
                string?(object, "path")
            end
            updated_file(path, move || path, diff)
          end
        end
        return if patches.empty?
        patches.join("\n")
      end

      private def edit(input : Hash(String, JSON::Any)) : String?
        path = file_path(input)
        return unless path
        old_text = string?(input, "old_string") ||
                   string?(input, "oldString")
        new_text = string?(input, "new_string") ||
                   string?(input, "newString")
        return unless old_text || new_text

        replacement(path, old_text, new_text)
      end

      private def write(input : Hash(String, JSON::Any)) : String?
        path = file_path(input)
        content = string?(input, "content")
        return unless path && content
        new_file(path, content)
      end

      private def multi_edit(input : Hash(String, JSON::Any)) : String?
        path = file_path(input)
        edits = input["edits"]?.try(&.as_a?)
        return unless path && edits

        patches = edits.compact_map do |node|
          item = node.as_h?
          next unless item
          old_text = string?(item, "old_string") ||
                     string?(item, "oldString") || ""
          new_text = string?(item, "new_string") ||
                     string?(item, "newString") || ""
          replacement(path, old_text, new_text)
        end
        return if patches.empty?
        patches.join("\n")
      end

      private def replacement(
        path : String,
        old_text : String?,
        new_text : String?,
      ) : String
        old_text ||= ""
        new_text ||= ""
        old_count = line_count(old_text)
        new_count = line_count(new_text)
        String.build do |io|
          file_header(io, path, path)
          io << "--- a/" << safe_path(path) << '\n'
          io << "+++ b/" << safe_path(path) << '\n'
          io << "@@ -1," << old_count << " +1," << new_count << " @@\n"
          prefix_lines(io, old_text, '-')
          prefix_lines(io, new_text, '+')
        end
      end

      private def new_file(path : String, content : String) : String
        String.build do |io|
          file_header(io, path, path)
          io << "new file mode 100644\n"
          io << "--- /dev/null\n"
          io << "+++ b/" << safe_path(path) << '\n'
          io << "@@ -0,0 +1," << line_count(content) << " @@\n"
          prefix_lines(io, content, '+')
        end
      end

      private def deleted_file(path : String, content : String) : String
        String.build do |io|
          file_header(io, path, path)
          io << "deleted file mode 100644\n"
          io << "--- a/" << safe_path(path) << '\n'
          io << "+++ /dev/null\n"
          io << "@@ -1," << line_count(content) << " +0,0 @@\n"
          prefix_lines(io, content, '-')
        end
      end

      private def updated_file(
        old_path : String,
        new_path : String,
        diff : String,
      ) : String
        return diff if diff.starts_with?("diff --git ")

        String.build do |io|
          file_header(io, old_path, new_path)
          if old_path != new_path
            io << "rename from " << safe_path(old_path) << '\n'
            io << "rename to " << safe_path(new_path) << '\n'
          end
          unless diff.starts_with?("--- ")
            io << "--- a/" << safe_path(old_path) << '\n'
            io << "+++ b/" << safe_path(new_path) << '\n'
          end
          io << diff
          io << '\n' unless diff.ends_with?('\n')
        end
      end

      private def file_header(
        io : IO,
        old_path : String,
        new_path : String,
      ) : Nil
        io << "diff --git a/" << safe_path(old_path)
        io << " b/" << safe_path(new_path) << '\n'
      end

      private def prefix_lines(io : IO, text : String, prefix : Char) : Nil
        return if text.empty?

        text.each_line(chomp: true) do |line|
          io << prefix << line << '\n'
        end
      end

      private def line_count(text : String) : Int32
        return 0 if text.empty?
        count = text.count('\n')
        text.ends_with?('\n') ? count : count + 1
      end

      private def file_path(input : Hash(String, JSON::Any)) : String?
        string?(input, "file_path") ||
          string?(input, "filePath") ||
          string?(input, "path")
      end

      private def string?(
        input : Hash(String, JSON::Any),
        key : String,
      ) : String?
        input[key]?.try(&.as_s?)
      end

      private def safe_path(path : String) : String
        path.gsub({'\n' => ' ', '\r' => ' ', '\t' => ' '})
      end

      private def truncate(patch : String) : String
        return patch if patch.bytesize <= LIMIT

        patch = patch.byte_slice(0, LIMIT)
        until patch.valid_encoding?
          patch = patch.byte_slice(0, patch.bytesize - 1)
        end
        patch + "\n… diff truncated …"
      end
    end
  end
end
