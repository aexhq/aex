import bashTool from "./dist/bash.js";
import editTool from "./dist/edit.js";
import globTool from "./dist/glob.js";
import grepTool from "./dist/grep.js";
import lsTool from "./dist/ls.js";
import readTool from "./dist/read.js";
import todoTool from "./dist/todo.js";
import writeTool from "./dist/write.js";

export const bash = () => bashTool;
export const edit = () => editTool;
export const glob = () => globTool;
export const grep = () => grepTool;
export const ls = () => lsTool;
export const read = () => readTool;
export const todo = () => todoTool;
export const write = () => writeTool;
