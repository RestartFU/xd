require "json"

module Xd
  module Agent
    # Builds display-only unified patches from tool payloads. This path needs
    # no repository, Git executable, filesystem read, or post-tool snapshot.
    module ToolDiff
      extend self

      PREFIX            = "file_change\n"
      LIMIT             = 256 * 1024
      BUILD_LIMIT       = LIMIT + 1
      TRUNCATION_NOTICE = "… diff truncated …"

      # Keeps display-only patch construction bounded before the final
      # truncation pass. Tool payloads can contain multi-megabyte files.
      private class PatchBuilder
        getter truncated = false

        @io = IO::Memory.new
        @bytesize = 0

        def initialize(@limit : Int32 = BUILD_LIMIT)
        end

        def <<(value : String) : self
          return self if value.empty?

          remaining = @limit - @bytesize
          if remaining <= 0
            @truncated = true
            return self
          end

          if value.bytesize <= remaining
            @io << value
            @bytesize += value.bytesize
            return self
          end

          part = value.byte_slice(0, remaining)
          until part.valid_encoding?
            part = part.byte_slice(0, part.bytesize - 1)
          end
          @io << part
          @bytesize += part.bytesize
          @truncated = true
          self
        end

        def <<(value) : self
          self << value.to_s
        end

        def full? : Bool
          @truncated || @bytesize >= @limit
        end

        def mark_truncated : Nil
          @truncated = true
        end

        def remaining : Int32
          Math.max(@limit - @bytesize, 0)
        end

        def to_s : String
          result = @io.to_s
          if @truncated && result.bytesize <= LIMIT
            result += "\n#{TRUNCATION_NOTICE}"
          end
          result
        end
      end

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
                when "notebookedit", "notebook_edit"
                  notebook_edit(input)
                when "apply_patch", "patch"
                  patch(input)
                else
                  nil
                end
        return unless patch

        patch = truncate(patch).rstrip
        return if patch.empty?
        PREFIX + patch
      end

      def wrap_unified(patch : String?) : String?
        return unless patch

        patch = truncate(patch).rstrip
        return if patch.empty? || !patch.starts_with?("diff --git ")
        PREFIX + patch
      end

      private def codex(input : Hash(String, JSON::Any)) : String?
        changes = input["changes"]?.try(&.as_a?)
        return unless changes

        output = PatchBuilder.new
        rendered = false
        changes.each do |node|
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

          patch = case kind
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
          output << '\n' if rendered
          output << patch
          rendered = true
          break if output.full?
        end
        return unless rendered
        output.to_s
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

        output = PatchBuilder.new
        rendered = false
        edits.each do |node|
          item = node.as_h?
          next unless item
          old_text = string?(item, "old_string") ||
                     string?(item, "oldString") || ""
          new_text = string?(item, "new_string") ||
                     string?(item, "newString") || ""
          output << '\n' if rendered
          output << replacement(path, old_text, new_text)
          rendered = true
          break if output.full?
        end
        return unless rendered
        output.to_s
      end

      private def notebook_edit(input : Hash(String, JSON::Any)) : String?
        path = file_path(input)
        return unless path
        old_text = string?(input, "old_source") ||
                   string?(input, "oldSource")
        new_text = string?(input, "new_source") ||
                   string?(input, "newSource")
        return unless old_text || new_text

        replacement(path, old_text, new_text)
      end

      private def patch(input : Hash(String, JSON::Any)) : String?
        raw = string?(input, "patch") ||
              string?(input, "diff") ||
              string?(input, "input")
        return unless raw
        return raw if raw.starts_with?("diff --git ")

        apply_patch(raw)
      end

      private def apply_patch(raw : String) : String?
        source = raw
        input_truncated = raw.bytesize > BUILD_LIMIT
        if input_truncated
          source = raw.byte_slice(0, BUILD_LIMIT)
          until source.valid_encoding?
            source = source.byte_slice(0, source.bytesize - 1)
          end
          source += "\n*** End Patch"
        end

        lines = source.lines(chomp: true)
        return unless lines.first? == "*** Begin Patch"
        return unless lines.last? == "*** End Patch"

        output = PatchBuilder.new
        rendered_any = false
        index = 1
        finish = lines.size - 1
        while index < finish
          header = lines[index]
          kind, path = apply_header(header)
          return unless kind && path
          index += 1

          move : String? = nil
          if kind == :update && index < finish &&
             lines[index].starts_with?("*** Move to: ")
            move = apply_path(lines[index], "*** Move to: ")
            return unless move
            index += 1
          end

          start = index
          while index < finish && !apply_file_header?(lines[index])
            index += 1
          end
          body = lines[start...index]
          if body.last? == "*** End of File"
            body = body[0...-1]
          end
          rendered = case kind
                     when :add
                       apply_add(path, body)
                     when :delete
                       apply_delete(path, body)
                     else
                       apply_update(path, move || path, body)
                     end
          return unless rendered
          output << '\n' if rendered_any
          output << rendered
          rendered_any = true
          break if output.full?
        end
        return unless rendered_any

        result = output.to_s
        if input_truncated && result.bytesize <= LIMIT
          result = result.rstrip + "\n#{TRUNCATION_NOTICE}"
        end
        result
      end

      private def apply_header(line : String) : Tuple(Symbol?, String?)
        {
          {"*** Add File: ", :add},
          {"*** Delete File: ", :delete},
          {"*** Update File: ", :update},
        }.each do |entry|
          prefix, kind = entry
          if line.starts_with?(prefix)
            return {kind, apply_path(line, prefix)}
          end
        end
        {nil, nil}
      end

      private def apply_path(line : String, prefix : String) : String?
        path = line.byte_slice(prefix.bytesize).strip
        path unless path.empty?
      end

      private def apply_file_header?(line : String) : Bool
        line.starts_with?("*** Add File: ") ||
          line.starts_with?("*** Delete File: ") ||
          line.starts_with?("*** Update File: ")
      end

      private def apply_add(path : String, body : Array(String)) : String?
        return if body.empty?
        return unless body.all?(&.starts_with?('+'))

        content = body.map { |line| line.byte_slice(1) }.join('\n') + '\n'
        new_file(path, content)
      end

      private def apply_delete(
        path : String,
        body : Array(String),
      ) : String?
        return deleted_file(path, "") if body.empty?
        return unless body.all?(&.starts_with?('-'))

        content = body.map { |line| line.byte_slice(1) }.join('\n') + '\n'
        deleted_file(path, content)
      end

      private def apply_update(
        old_path : String,
        new_path : String,
        body : Array(String),
      ) : String?
        return if body.empty? && old_path == new_path

        hunks = [] of Array(String)
        current : Array(String)? = nil
        body.each do |line|
          if line.starts_with?("@@")
            hunks << current if current
            current = [] of String
          elsif rows = current
            return if line.empty?
            return unless {' ', '+', '-'}.includes?(line[0])
            rows << line
          else
            return
          end
        end
        hunks << current if current
        return if hunks.empty? && old_path == new_path
        return if hunks.any?(&.empty?)

        io = PatchBuilder.new
        file_header(io, old_path, new_path)
        if old_path != new_path
          io << "rename from " << safe_path(old_path) << '\n'
          io << "rename to " << safe_path(new_path) << '\n'
        end
        unless hunks.empty?
          io << "--- a/" << safe_path(old_path) << '\n'
          io << "+++ b/" << safe_path(new_path) << '\n'
          hunks.each do |hunk|
            old_count = hunk.count { |line| !line.starts_with?('+') }
            new_count = hunk.count { |line| !line.starts_with?('-') }
            old_start = old_count == 0 ? 0 : 1
            new_start = new_count == 0 ? 0 : 1
            io << "@@ -" << old_start << ',' << old_count
            io << " +" << new_start << ',' << new_count << " @@\n"
            hunk.each do |line|
              io << line << '\n'
              break if io.full?
            end
            break if io.full?
          end
        end
        io.to_s
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
        io = PatchBuilder.new
        file_header(io, path, path)
        io << "--- a/" << safe_path(path) << '\n'
        io << "+++ b/" << safe_path(path) << '\n'
        io << "@@ -1," << old_count << " +1," << new_count << " @@\n"
        prefix_lines(io, old_text, '-')
        prefix_lines(io, new_text, '+')
        io.to_s
      end

      private def new_file(path : String, content : String) : String
        io = PatchBuilder.new
        file_header(io, path, path)
        io << "new file mode 100644\n"
        io << "--- /dev/null\n"
        io << "+++ b/" << safe_path(path) << '\n'
        io << "@@ -0,0 +1," << line_count(content) << " @@\n"
        prefix_lines(io, content, '+')
        io.to_s
      end

      private def deleted_file(path : String, content : String) : String
        io = PatchBuilder.new
        file_header(io, path, path)
        io << "deleted file mode 100644\n"
        io << "--- a/" << safe_path(path) << '\n'
        io << "+++ /dev/null\n"
        io << "@@ -1," << line_count(content) << " +0,0 @@\n"
        prefix_lines(io, content, '-')
        io.to_s
      end

      private def updated_file(
        old_path : String,
        new_path : String,
        diff : String,
      ) : String
        return diff if diff.starts_with?("diff --git ")

        io = PatchBuilder.new
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
        io.to_s
      end

      private def file_header(
        io : PatchBuilder,
        old_path : String,
        new_path : String,
      ) : Nil
        io << "diff --git a/" << safe_path(old_path)
        io << " b/" << safe_path(new_path) << '\n'
      end

      private def prefix_lines(
        io : PatchBuilder,
        text : String,
        prefix : Char,
      ) : Nil
        return if text.empty?

        sample = text
        if text.bytesize > io.remaining
          sample = text.byte_slice(0, io.remaining)
          until sample.valid_encoding?
            sample = sample.byte_slice(0, sample.bytesize - 1)
          end
        end
        sample.each_line(chomp: true) do |line|
          io << prefix << line << '\n'
          break if io.full?
        end
        io.mark_truncated if sample.bytesize < text.bytesize
      end

      private def line_count(text : String) : Int32
        return 0 if text.empty?

        bytes = text.to_slice
        finish = Math.min(bytes.size, BUILD_LIMIT)
        count = 0
        index = 0
        while index < finish
          count += 1 if bytes[index] == '\n'.ord
          index += 1
        end
        finish > 0 && bytes[finish - 1] == '\n'.ord ? count : count + 1
      end

      private def file_path(input : Hash(String, JSON::Any)) : String?
        string?(input, "file_path") ||
          string?(input, "filePath") ||
          string?(input, "notebook_path") ||
          string?(input, "notebookPath") ||
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
        patch + "\n#{TRUNCATION_NOTICE}"
      end
    end
  end
end
