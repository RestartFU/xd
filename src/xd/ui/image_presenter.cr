require "base64"
require "gtk4"
require "../daemon/endpoint"
require "./adw"

module Xd
  module UI
    # One daemon-backed image path for local Unix and paired TLS endpoints.
    # Endpoint transport never changes preview, viewer, or error behavior.
    class ImagePresenter
      INLINE_MAX_WIDTH  =  168
      INLINE_MAX_HEIGHT =   96
      VIEWER_MAX_WIDTH  = 1920
      VIEWER_MAX_HEIGHT = 1200
      MAX_IMAGE_BYTES   = 10 * 1024 * 1024

      def preview(
        endpoint : Daemon::Endpoint,
        path : String,
        number : Int32,
      ) : Gtk::Widget
        preview = Gtk::Box.new(:vertical, 4)
        stack = Gtk::Stack.new
        picture = Gtk::Picture.new
        button = Gtk::Button.new

        spinner = Gtk::Spinner.new
        spinner.start
        loading = Gtk::Box.new(:horizontal, 6)
        loading.append(spinner)
        loading.append(Gtk::Label.new("Loading image…"))
        loading.add_css_class("dim-label")

        unavailable = Gtk::Label.new("Preview unavailable")
        unavailable.add_css_class("dim-label")

        stack.add_named(loading, "loading")
        stack.add_named(picture, "picture")
        stack.add_named(unavailable, "unavailable")
        stack.hhomogeneous = false
        stack.vhomogeneous = false
        stack.transition_type = :none
        stack.visible_child_name = "loading"
        stack.add_css_class("xd-inline-image")
        stack.halign = :start
        stack.valign = :start
        stack.vexpand = false

        button.child = stack
        button.add_css_class("flat")
        button.add_css_class("xd-image-button")
        button.halign = :start
        button.valign = :start
        button.tooltip_text = "Open image"
        button.clicked_signal.connect do
          open(endpoint, path, button)
        end

        label = Gtk::Label.new("")
        label.markup = %(<a href="image">Image ##{number}</a>)
        label.xalign = 0_f32
        label.add_css_class("caption")
        label.activate_link_signal.connect do |_uri|
          open(endpoint, path, label)
          true
        end

        preview.append(button)
        preview.append(label)
        preview.halign = :start
        preview.valign = :start
        preview.vexpand = false

        fetch(endpoint, path, true) do |body|
          texture = body.try do |response|
            self.class.texture_from(
              response,
              INLINE_MAX_WIDTH,
              INLINE_MAX_HEIGHT
            )
          end
          if texture
            picture.paintable = texture
            picture.content_fit = :scale_down
            picture.set_size_request(texture.width, texture.height)
            stack.set_size_request(texture.width, texture.height)
            stack.visible_child_name = "picture"
          else
            stack.visible_child_name = "unavailable"
          end
        end

        preview
      end

      def self.texture_from_png(
        data : Bytes,
        max_width : Int32,
        max_height : Int32,
      ) : Gdk::Texture?
        return if data.empty? || data.size > MAX_IMAGE_BYTES

        loader = GdkPixbuf::PixbufLoader.new_with_type("png")
        loader.size_prepared_signal.connect do |width, height|
          next if width <= 0 || height <= 0

          scale = {
            1.0,
            max_width.to_f64 / width,
            max_height.to_f64 / height,
          }.min
          loader.set_size(
            Math.max(1, (width * scale).to_i),
            Math.max(1, (height * scale).to_i)
          )
        end
        loader.write(data)
        loader.close
        pixbuf = loader.pixbuf
        pixbuf ? Gdk::Texture.new_for_pixbuf(pixbuf) : nil
      rescue GLib::Error
        nil
      end

      def self.texture_from(
        body : Hash(String, JSON::Any),
        max_width : Int32,
        max_height : Int32,
      ) : Gdk::Texture?
        return unless body["mime"]?.try(&.as_s?) == "image/png"
        encoded = body["data"]?.try(&.as_s?) || return
        encoded_limit = ((MAX_IMAGE_BYTES + 2) // 3) * 4
        return if encoded.bytesize > encoded_limit

        texture_from_png(
          Base64.decode(encoded),
          max_width,
          max_height
        )
      rescue Base64::Error
        nil
      end

      private def open(
        endpoint : Daemon::Endpoint,
        path : String,
        anchor : Gtk::Widget,
      ) : Nil
        dialog = Adw::Dialog.new
        overlay = Gtk::Overlay.new
        stack = Gtk::Stack.new
        picture = Gtk::Picture.new

        picture.content_fit = :contain
        picture.halign = :center
        picture.valign = :center
        picture.margin_top = 24
        picture.margin_bottom = 24
        picture.margin_start = 24
        picture.margin_end = 24

        spinner = Gtk::Spinner.new
        spinner.start
        spinner.halign = :center
        spinner.valign = :center
        unavailable = Gtk::Label.new("Image unavailable")
        unavailable.add_css_class("dim-label")

        stack.add_named(spinner, "loading")
        stack.add_named(picture, "picture")
        stack.add_named(unavailable, "unavailable")
        stack.transition_type = :none
        stack.visible_child_name = "loading"
        stack.hexpand = true
        stack.vexpand = true
        stack.add_css_class("xd-image-viewer")

        background_click = Gtk::GestureClick.new
        background_click.pressed_signal.connect do |_count, x, y|
          target = stack.pick(x, y, Gtk::PickFlags::Default)
          unless target && target.to_unsafe == picture.to_unsafe
            dialog.close
          end
        end
        stack.add_controller(background_click)

        close = Gtk::Button.new_from_icon_name("window-close-symbolic")
        close.add_css_class("circular")
        close.add_css_class("osd")
        close.halign = :end
        close.valign = :start
        close.margin_top = 12
        close.margin_end = 12
        close.tooltip_text = "Close"
        close.clicked_signal.connect { dialog.close }

        overlay.child = stack
        overlay.add_overlay(close)

        dialog.title = "Image"
        dialog.content_width = 1100
        dialog.content_height = 720
        dialog.child = overlay
        dialog.add_css_class("xd-image-dialog")

        closed = false
        dialog.closed_signal.connect { closed = true }
        dialog.present(anchor)

        fetch(endpoint, path, false) do |body|
          unless closed
            texture = body.try do |response|
              self.class.texture_from(
                response,
                VIEWER_MAX_WIDTH,
                VIEWER_MAX_HEIGHT
              )
            end
            if texture
              picture.paintable = texture
              stack.visible_child_name = "picture"
            else
              stack.visible_child_name = "unavailable"
            end
          end
        end
      end

      private def fetch(
        endpoint : Daemon::Endpoint,
        path : String,
        preview : Bool,
        &callback : Hash(String, JSON::Any)? ->
      ) : Nil
        spawn do
          response : Hash(String, JSON::Any)? = nil
          begin
            response = endpoint.call({
              "op"      => JSON::Any.new("image-read"),
              "path"    => JSON::Any.new(path),
              "preview" => JSON::Any.new(preview),
            })
          rescue
          end
          GLib.idle_add do
            callback.call(response)
            false
          end
        end
      end
    end
  end
end
