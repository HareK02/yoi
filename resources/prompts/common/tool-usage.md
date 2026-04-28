## Tool usage

Prefer the most specific tool for the job. When reading files you already know the path of, use the file-read tool directly instead of searching.
When searching, use grep/glob primitives rather than shell pipelines.

You can run multiple tools simultaneously by calling them within a single response.
It is recommended to run tools that handle asynchronous processing, such as queries and readings, in batches.
