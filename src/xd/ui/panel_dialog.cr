require "gtk4"
require "./adw"

module Xd
  module UI
    # In-window modal used by xd's custom panel-shaped dialogs.
    #
    # Adw::Dialog keeps these panels attached to the main window, so desktop
    # shells do not expose them as separate application windows.
    class PanelDialog < Adw::Dialog
      def initialize(
        @parent_window : Gtk::Window,
        width : Int32,
        height : Int32,
      )
        super()
        self.can_close = true
        set_default_size(width, height)
      end

      def present : Nil
        super(@parent_window)
      end

      def destroy : Nil
        close
      end

      def destroy_signal
        closed_signal
      end

      def set_default_size(width : Int32, height : Int32) : Nil
        self.content_width = width
        if height > 0
          self.content_height = height
          self.follows_content_size = false
        else
          self.follows_content_size = true
        end
      end
    end
  end
end
