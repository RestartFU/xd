require "gtk4"

module Xd
  module UI
    # Shared presentation for a long-running item in the transcript. Workflows
    # and delegated agents supply different data, but the card, summary row,
    # status treatment, expandable body, and spacing stay identical.
    class RunCard
      STATUS_CLASSES = {
        "xd-workflow-running",
        "xd-workflow-success",
        "xd-workflow-failure",
        "xd-workflow-finished",
      }

      getter widget : Gtk::Box
      getter summary : Gtk::Box
      getter spinner : Gtk::Spinner
      getter name : Gtk::Label
      getter elapsed : Gtk::Label
      getter status : Gtk::Label
      getter items : Gtk::Box

      def initialize(
        title : String,
        footer : String? = nil,
        heading_suffix : Gtk::Widget? = nil,
      )
        heading_text = Gtk::Label.new(title)
        heading_text.xalign = 0_f32
        heading_text.add_css_class("title")

        heading = Gtk::Box.new(:horizontal, 4)
        heading.append(heading_text)
        heading.append(heading_suffix) if heading_suffix

        @spinner = Gtk::Spinner.new
        @spinner.visible = true
        @spinner.spinning = true

        @name = Gtk::Label.new("")
        @name.xalign = 0_f32
        @name.hexpand = true
        @name.add_css_class("xd-workflow-name")

        @elapsed = Gtk::Label.new("")
        @elapsed.xalign = 1_f32
        @elapsed.visible = false
        @elapsed.add_css_class("xd-workflow-elapsed")

        @status = Gtk::Label.new("")
        @status.xalign = 1_f32
        @status.visible = false
        @status.add_css_class("xd-workflow-status")
        @status.add_css_class("xd-workflow-running")

        @summary = Gtk::Box.new(:horizontal, 8)
        @summary.append(@spinner)
        @summary.append(@name)
        @summary.append(@elapsed)
        @summary.append(@status)

        @items = Gtk::Box.new(:vertical, 6)
        @items.add_css_class("xd-workflow-jobs")

        @widget = Gtk::Box.new(:vertical, 7)
        @widget.add_css_class("xd-workflow")
        @widget.append(heading)
        @widget.append(@summary)
        @widget.append(@items)

        if footer
          footer_label = Gtk::Label.new(footer)
          footer_label.xalign = 0_f32
          footer_label.add_css_class("dim-label")
          @widget.append(footer_label)
        end
      end

      def apply_status_class(css_class : String) : Nil
        STATUS_CLASSES.each { |name| @status.remove_css_class(name) }
        @status.add_css_class(css_class)
      end
    end
  end
end
