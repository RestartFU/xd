require "json"
require "gtk4"
require "./panel_call"

module Xd
  module UI
    # Windows omits terminal control until a native ConPTY-backed terminal
    # widget can provide the same lifecycle guarantees as VTE.
    class TerminalPanel
      getter widget : Gtk::Box

      # Nothing measures anything here, but the window saves and restores this
      # the same way on every platform.
      property geometry : Tuple(Int64, Int64)?

      def initialize(
        @request : PanelCall,
        @on_empty : Proc(Nil),
      )
        @widget = Gtk::Box.new(:vertical, 0)
      end

      def select_chat(_chat_id : String?, _view_key : String?) : Nil
      end

      def activate(_focus : Bool = true) : Nil
      end

      def handle_event(_event : Hash(String, JSON::Any)) : Nil
      end

      def remote_connection_changed(
        _connected : Bool,
        _error : String?,
      ) : Nil
      end
    end
  end
end
