require "json"
require "./operation"

module Xd
  module Protocol
    class Error < Exception
    end

    class Request
      getter operation : Operation
      getter body : Hash(String, JSON::Any)

      def initialize(@operation, @body)
      end

      def self.parse(line : String) : self
        root = JSON.parse(line)
        body = root.as_h?
        raise Error.new("Not a JSON object") unless body

        name = body["op"]?.try(&.as_s?)
        operation = name.try { |value| Operation.from_wire?(value) }
        raise Error.new("Unknown op") unless operation

        new(operation, body)
      rescue JSON::ParseException
        raise Error.new("Not a JSON object")
      end

      def string?(name : String) : String?
        @body[name]?.try(&.as_s?)
      end
    end

    class Response
      getter body : Hash(String, JSON::Any)

      private def initialize(@body)
      end

      def self.ok(fields = {} of String => JSON::Any) : self
        body = {"ok" => JSON::Any.new(true)}
        fields.each { |name, value| body[name] = value }
        new(body)
      end

      def self.error(message : String) : self
        new({
          "ok"    => JSON::Any.new(false),
          "error" => JSON::Any.new(message),
        })
      end

      def success? : Bool
        @body["ok"].as_bool
      end

      def [](name : String) : JSON::Any
        @body[name]
      end

      def to_json(io : IO) : Nil
        @body.to_json(io)
      end

      def to_json : String
        String.build { |io| to_json(io) }
      end
    end
  end
end
