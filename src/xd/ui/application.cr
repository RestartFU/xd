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
      Gtk::StyleContext.add_provider_for_display(
        display,
        provider,
        (Gtk::STYLE_PROVIDER_PRIORITY_USER + 1).to_u32
      )
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
