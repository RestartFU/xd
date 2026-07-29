require "gtk4"
require "../version"
require "./runtime"
require "./window"

module Xd
  module UI
    STYLE = <<-CSS
      window {
        background: #0a0a0c;
        color: #f2f2f4;
        font-family: "Inter", "Cantarell", sans-serif;
      }
      .xd-sidebar { background: #060607; }
      .xd-chat { background: #0a0a0c; }
      .xd-tool-panel { background: #08080a; }
      .xd-terminal {
        background: #060607;
        color: #f2f2f4;
        padding: 8px;
      }
      .xd-sidebar button.flat {
        background: transparent;
        border: 0;
        border-radius: 8px;
        padding: 7px 9px;
      }
      .xd-sidebar button.flat:hover { background: alpha(#ffffff, 0.07); }
      .xd-message {
        border-radius: 12px;
        padding: 12px 14px;
      }
      .xd-message-user { background: alpha(#ffffff, 0.07); }
      .xd-message-assistant { background: transparent; }
      .xd-message-tool {
        background: alpha(#3584e4, 0.10);
        color: #b8d9ff;
      }
      .xd-message-diff {
        background: alpha(#33d17a, 0.07);
        color: #d7f8e5;
        font-family: "JetBrains Mono", monospace;
        font-size: 0.9em;
      }
      .xd-message-error {
        background: alpha(#e01b24, 0.12);
        color: #ffb4ab;
      }
      .xd-ask {
        margin: 0 14px 8px 14px;
      }
      .xd-choice {
        background: alpha(#ffffff, 0.045);
        border: 1px solid alpha(#ffffff, 0.10);
        border-radius: 9px;
        padding: 8px 10px;
      }
      .xd-choice:hover {
        background: alpha(#ffffff, 0.09);
      }
      .xd-controls button {
        min-height: 26px;
        padding: 3px 8px;
      }
      .xd-controls button.flat:hover {
        background: alpha(#ffffff, 0.07);
      }
      .xd-queue {
        background: alpha(#f6d32d, 0.06);
        border-radius: 10px;
        padding: 6px 8px;
      }
      .xd-queue-row button {
        min-height: 24px;
        padding: 2px 7px;
      }
      .xd-composer entry {
        border-radius: 12px;
        padding: 10px 12px;
      }
      CSS

    extend self

    def run : Int32
      application = Gtk::Application.new(
        APP_ID,
        Gio::ApplicationFlags::None
      )
      runtime : Runtime? = nil
      window : Window? = nil

      application.activate_signal.connect do
        begin
          install_style
          runtime ||= Runtime.new
          window ||= Window.new(application, runtime.not_nil!.client)
          window.not_nil!.present
        rescue error
          show_startup_error(application, error)
        end
      end
      application.shutdown_signal.connect do
        runtime.try(&.close)
      end

      # GTK owns main loop; brief yields let Crystal's socket fibers consume
      # daemon events without a second thread or second application model.
      GLib.timeout(10.milliseconds) do
        Fiber.yield
        true
      end

      application.run
    end

    private def install_style : Nil
      display = Gdk::Display.default
      return unless display

      provider = Gtk::CssProvider.new
      provider.load_from_string(STYLE)
      Gtk::StyleContext.add_provider_for_display(display, provider, 800_u32)
    end

    private def show_startup_error(
      application : Gtk::Application,
      error : Exception,
    ) : Nil
      window = Gtk::ApplicationWindow.new(application)
      window.title = "xd"
      window.set_default_size(560, 180)
      label = Gtk::Label.new(
        "xd could not start\n\n#{error.message || error.class.name}"
      )
      label.wrap = true
      label.margin_top = 24
      label.margin_bottom = 24
      label.margin_start = 24
      label.margin_end = 24
      window.child = label
      window.present
    end
  end
end
