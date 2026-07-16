class ModelError(RuntimeError):
    RETRYABLE_CATEGORIES = frozenset(
        {"transient_network", "timeout", "rate_limit", "server_error"}
    )

    def __init__(self, message: str, *, category: str = "invalid_response"):
        super().__init__(message)
        self.category = category

    @property
    def retryable(self) -> bool:
        return self.category in self.RETRYABLE_CATEGORIES
