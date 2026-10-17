A light command such as nextest run is now judged by its own memory peak, recorded per repository and command, and no longer by the heavier peaks of cargo dev ci in the same repository.
