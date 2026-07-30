require "gtk4"
require "../version"
require "./adw"
require "./runtime"
require "./style"
require "./window"

module Xd
  module UI
    extend self

    def run : Int32
      application = Adw::Application.new(
        APP_ID,
        Gio::ApplicationFlags::None
      )
      runtime : Runtime? = nil
      window : Window? = nil
      startup_window : Adw::ApplicationWindow? = nil

      application.activate_signal.connect do
        begin
          application.style_manager.color_scheme =
            Adw::ColorScheme::ForceDark
          install_style
          runtime ||= Runtime.new
          window ||= Window.new(
            application,
            runtime.not_nil!.client,
            runtime.not_nil!.remote
          )
          window.not_nil!.present
        rescue error
          startup_window ||= show_startup_error(application, error)
          startup_window.not_nil!.present
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
      Gtk::StyleContext.add_provider_for_display(
        display,
        provider,
        (Gtk::STYLE_PROVIDER_PRIORITY_USER + 1).to_u32
      )
    end

    private def show_startup_error(
      application : Gtk::Application,
      error : Exception,
    ) : Adw::ApplicationWindow
      window = Adw::ApplicationWindow.new(application: application)
      window.title = "xd"
      window.set_default_size(1100, 720)

      # Match C startup failure state: normal app surface remains behind one
      # modal explanation, but no half-working sidebar or chat is constructed.
      split = Gtk::Paned.new(:horizontal)
      split.position = 280
      split.resize_start_child = false
      split.shrink_start_child = false
      split.resize_end_child = true
      split.shrink_end_child = false
      window.content = split
      window.present

      heading = if error.is_a?(Storage::Error)
                  "Cannot Open the Chat Database"
                else
                  "Cannot Start xd"
                end
      dialog = Adw::AlertDialog.new(
        heading: heading,
        body: error.message || error.class.name
      )
      dialog.add_response("quit", "Quit")
      dialog.default_response = "quit"
      dialog.close_response = "quit"
      dialog.choose(window, nil) do |_source, result|
        dialog.choose_finish(result)
        window.destroy
        application.quit
      end
      window
    end
  end
end
