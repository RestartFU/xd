require "gtk4"
require "../agent/workflow_run"

module Xd
  module UI
    class WorkflowCard
      POLL_INTERVAL  = 10.seconds
      RETRY_INTERVAL = 1.minute
      STATUS_CLASSES = {
        "xd-workflow-running",
        "xd-workflow-success",
        "xd-workflow-failure",
        "xd-workflow-finished",
      }

      getter widget : Gtk::Box

      def initialize(
        @run : Agent::WorkflowRun::Run,
        resolver : Proc(Agent::WorkflowRun::Status)? = nil,
      )
        @resolver = resolver || -> { Agent::WorkflowRun.fetch_status(@run) }
        @closed = false
        @generation = 0_i64
        @source_id = 0_u32

        heading_text = Gtk::Label.new("GitHub Actions ·")
        heading_text.xalign = 0_f32
        heading_text.add_css_class("title")
        run_link = Gtk::LinkButton.new_with_label(@run.url, "##{@run.id}")
        run_link.add_css_class("flat")
        run_link.add_css_class("xd-workflow-run-link")
        run_link.tooltip_text = "Open run ##{@run.id} on GitHub"
        heading = Gtk::Box.new(:horizontal, 4)
        heading.append(heading_text)
        heading.append(run_link)

        @run_spinner = Gtk::Spinner.new
        @run_spinner.visible = true
        @run_spinner.spinning = true

        @run_name = Gtk::Label.new("")
        @run_name.xalign = 0_f32
        @run_name.hexpand = true
        @run_name.add_css_class("xd-workflow-name")

        @run_status = Gtk::Label.new("")
        @run_status.xalign = 1_f32
        @run_status.visible = false
        @run_status.add_css_class("xd-workflow-status")
        @run_status.add_css_class("xd-workflow-running")

        summary = Gtk::Box.new(:horizontal, 8)
        summary.append(@run_spinner)
        summary.append(@run_name)
        summary.append(@run_status)

        @jobs = Gtk::Box.new(:vertical, 6)
        @jobs.add_css_class("xd-workflow-jobs")

        repository = Gtk::Label.new(@run.repository)
        repository.xalign = 0_f32
        repository.add_css_class("dim-label")

        @widget = Gtk::Box.new(:vertical, 7)
        @widget.add_css_class("xd-workflow")
        @widget.append(heading)
        @widget.append(summary)
        @widget.append(@jobs)
        @widget.append(repository)
        check
      end

      def close : Nil
        return if @closed
        @closed = true
        @generation += 1
        unless @source_id == 0
          GLib.source_remove(@source_id)
          @source_id = 0_u32
        end
      end

      private def check : Nil
        return if @closed
        @generation += 1
        generation = @generation
        Fiber::ExecutionContext::Isolated.new(
          "xd workflow #{@run.id}"
        ) do
          snapshot : Agent::WorkflowRun::Status? = nil
          begin
            snapshot = @resolver.call
          rescue Agent::WorkflowRun::StatusError | IO::Error
          end
          GLib.idle_add do
            finish_check(snapshot, generation)
            false
          end
        end
      end

      private def finish_check(
        snapshot : Agent::WorkflowRun::Status?,
        generation : Int64,
      ) : Nil
        return if @closed || generation != @generation
        if snapshot
          set_status(snapshot)
          render_jobs(snapshot.jobs)
          schedule(POLL_INTERVAL) unless snapshot.terminal?
        else
          set_unavailable
          render_jobs([] of Agent::WorkflowRun::Job)
          schedule(RETRY_INTERVAL)
        end
      end

      private def set_status(
        snapshot : Agent::WorkflowRun::Status,
      ) : Nil
        terminal = snapshot.terminal?
        @run_name.text = snapshot.name.empty? ? "Workflow" : snapshot.name
        @run_spinner.visible = !terminal
        @run_spinner.spinning = !terminal
        @run_status.visible = terminal
        @run_status.text = snapshot.result_label if terminal
        apply_status_class(snapshot.css_class)
      end

      private def set_unavailable : Nil
        @run_name.text = "Workflow"
        @run_spinner.visible = false
        @run_spinner.spinning = false
        @run_status.visible = true
        @run_status.text = "Status unavailable"
        apply_status_class("xd-workflow-finished")
      end

      private def apply_status_class(css_class : String) : Nil
        STATUS_CLASSES.each { |name| @run_status.remove_css_class(name) }
        @run_status.add_css_class(css_class)
      end

      private def render_jobs(
        jobs : Array(Agent::WorkflowRun::Job),
      ) : Nil
        @jobs.children.each { |child| @jobs.remove(child) }
        if jobs.empty?
          empty = Gtk::Label.new("No jobs reported yet")
          empty.xalign = 0_f32
          empty.add_css_class("dim-label")
          @jobs.append(empty)
          return
        end

        jobs.each { |job| @jobs.append(job_widget(job)) }
      end

      private def job_widget(
        job : Agent::WorkflowRun::Job,
      ) : Gtk::Box
        box = Gtk::Box.new(:vertical, 4)
        box.add_css_class("xd-workflow-job")

        header = Gtk::Box.new(:horizontal, 8)
        if job.terminal?
          icon_name = case job.conclusion
                      when "success" then "emblem-ok-symbolic"
                      when "failure", "timed_out", "startup_failure"
                        "dialog-error-symbolic"
                      else
                        "dialog-information-symbolic"
                      end
          icon = Gtk::Image.new_from_icon_name(icon_name)
          icon.valign = :center
          header.append(icon)
        else
          spinner = Gtk::Spinner.new
          spinner.spinning = true
          header.append(spinner)
        end

        name = Gtk::Label.new(job.name)
        name.xalign = 0_f32
        name.hexpand = true
        name.add_css_class("xd-workflow-job-name")
        header.append(name)

        if job.terminal?
          status = Gtk::Label.new(job.label)
          status.xalign = 1_f32
          status.add_css_class("xd-workflow-status")
          status.add_css_class(job.css_class)
          header.append(status)
        end
        box.append(header)

        if log = job.log
          detail = Gtk::Label.new(log)
          detail.xalign = 0_f32
          detail.wrap = true
          detail.selectable = true
          detail.tooltip_text = log
          detail.add_css_class("xd-workflow-log")
          box.append(detail)
        end
        box
      end

      private def schedule(delay : Time::Span) : Nil
        return if @closed
        @source_id = GLib.timeout(delay) do
          @source_id = 0_u32
          check
          false
        end
      end
    end
  end
end
