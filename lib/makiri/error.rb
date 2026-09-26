# frozen_string_literal: true

module Makiri
  # The error classes themselves are defined by the extension (init.rs); this
  # adds what only Ruby can do.
  class Error
    # +exc+ with +cause+ as its #cause, handed back for the extension to raise.
    #
    # Ruby sets #cause only when raising - there is no C API for it - so this
    # raises +exc+ with `cause:` and returns it. The backtrace that raise
    # recorded (this method's) is replaced with the caller's, so the error
    # points at the call that failed, as it would had the extension raised it
    # directly. `caller` is taken before the raise: in the rescue clause it
    # would include this method's own frame.
    def self.__with_cause(exc, cause)
      backtrace = caller
      begin
        raise exc, cause: cause
      rescue Exception => e # rubocop:disable Lint/RescueException
        e.set_backtrace(backtrace)
        e
      end
    end
    private_class_method :__with_cause
  end
end
